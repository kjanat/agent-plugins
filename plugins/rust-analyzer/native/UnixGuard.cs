using System.Buffers.Binary;
using System.ComponentModel;
using System.Diagnostics;
using System.Globalization;
using System.IO.Pipes;
using System.Reflection;
using System.Runtime.InteropServices;

namespace AgentPlugins.RustAnalyzer;

internal static partial class UnixGuard
{
    private const int NoSuchProcess = 3;

    internal static int Run(string executable, string[] arguments, ulong limitMiB)
    {
        using var control = new AnonymousPipeServerStream(PipeDirection.In, HandleInheritability.Inheritable);
        var start = new ProcessStartInfo(Environment.ProcessPath
            ?? throw new InvalidOperationException("Cannot locate .NET host."))
        {
            UseShellExecute = false,
        };
        start.ArgumentList.Add(Assembly.GetExecutingAssembly().Location);
        start.ArgumentList.Add("--guard-unix-child");
        start.ArgumentList.Add(Environment.ProcessId.ToString(CultureInfo.InvariantCulture));
        start.ArgumentList.Add(control.GetClientHandleAsString());
        start.ArgumentList.Add(executable);
        foreach (string argument in arguments) start.ArgumentList.Add(argument);
        using Process child = RustAnalyzerGuard.Start(start);
        control.DisposeLocalCopyOfClientHandle();
        bool owned = false;
        try
        {
            byte[] ready = new byte[1];
            control.ReadExactlyAsync(ready).AsTask().WaitAsync(TimeSpan.FromSeconds(5)).GetAwaiter().GetResult();
            owned = ready[0] == 1 && Group(child.Id) == child.Id && child.Id > 1;
            if (!owned) throw new InvalidOperationException("Child did not establish its own process group.");
            Console.Error.WriteLine("rust-guard: soft RSS watchdog; polling may overshoot the memory budget.");
            byte[] status = new byte[sizeof(int)];
            Task completed = control.ReadExactlyAsync(status).AsTask();
            ulong limitBytes = checked(limitMiB * 1024 * 1024);
            while (!completed.Wait(100))
            {
                if (child.HasExited) throw new IOException("Internal LSP supervisor exited unexpectedly.");
                if (ResidentBytes(child.Id) >= limitBytes)
                {
                    Console.Error.WriteLine($"rust-guard: LSP stopped at its {limitMiB} MiB RSS budget.");
                    return RustAnalyzerGuard.BudgetExitCode;
                }
            }
            return BinaryPrimitives.ReadInt32LittleEndian(status);
        }
        finally
        {
            // Leader stays alive until cleanup, including after the LSP exits. No PID reuse gap.
            int group = Group(child.Id);
            if (group == child.Id || (owned && group == -1 && Marshal.GetLastPInvokeError() == NoSuchProcess))
                KillGroup(child.Id);
            else if (!child.HasExited)
                child.Kill();
            child.WaitForExit(2000);
        }
    }

    internal static int RunChild(string[] arguments)
    {
        if (!(OperatingSystem.IsLinux() || OperatingSystem.IsMacOS()) || arguments.Length < 4
            || !int.TryParse(arguments[1], out int parent) || parent <= 1 || Parent() != parent)
            throw new ArgumentException("Invalid internal launcher invocation.");
        if (SetGroup(0, 0) != 0) throw new Win32Exception(Marshal.GetLastPInvokeError(), "setpgid");
        int group = Environment.ProcessId;
        if (group <= 1 || Group(0) != group)
            throw new InvalidOperationException("Failed to isolate process group.");
        try
        {
            using var control = new AnonymousPipeClientStream(PipeDirection.Out, arguments[2]);
            // Do not leak the private control channel into rust-analyzer or its descendants.
            int descriptor = control.SafePipeHandle.DangerousGetHandle().ToInt32();
            // FIOCLEX has no variadic argument, including on Apple ARM64.
            nuint closeOnExec = OperatingSystem.IsMacOS() ? 0x20006601u : 0x5451u;
            if (Ioctl(descriptor, closeOnExec) == -1)
                throw new Win32Exception(Marshal.GetLastPInvokeError(), "ioctl FIOCLEX");
            if (Parent() != parent) return RustAnalyzerGuard.FailureExitCode;
            control.WriteByte(1);
            control.Flush();
            int exitCode;
            try
            {
                var start = new ProcessStartInfo(arguments[3]) { UseShellExecute = false };
                foreach (string argument in arguments[4..]) start.ArgumentList.Add(argument);
                using Process server = RustAnalyzerGuard.Start(start);
                while (!server.WaitForExit(50)) CheckParent(parent, group);
                exitCode = server.ExitCode;
            }
            catch (Exception error)
            {
                Console.Error.WriteLine("rust-guard: could not launch or supervise LSP: " + error.Message);
                exitCode = RustAnalyzerGuard.FailureExitCode;
            }
            byte[] status = new byte[sizeof(int)];
            BinaryPrimitives.WriteInt32LittleEndian(status, exitCode);
            control.Write(status);
            control.Flush();
            // Keep ownership anchored while the outer supervisor removes remaining children.
            while (true)
            {
                CheckParent(parent, group);
                Thread.Sleep(50);
            }
        }
        catch (Exception error)
        {
            Console.Error.WriteLine("rust-guard: internal supervisor failed: " + error.Message);
            KillGroup(group);
            throw;
        }
        finally
        {
            if (Parent() != parent) KillGroup(group);
        }
    }

    private static void CheckParent(int parent, int group)
    {
        // getppid changes on reparenting, even if the original PID gets reused.
        if (Parent() != parent) KillGroup(group);
    }

    private static ulong ResidentBytes(int group)
    {
        ulong total = 0;
        Process[] processes = Process.GetProcesses();
        try
        {
            foreach (Process process in processes)
            {
                int membership = Group(process.Id);
                if (membership == -1)
                {
                    int error = Marshal.GetLastPInvokeError();
                    if (error == NoSuchProcess) continue;
                    throw new Win32Exception(error, "getpgid during memory accounting");
                }
                if (membership != group) continue;
                try { total = checked(total + (ulong)process.WorkingSet64); }
                catch (Exception error) when (error is InvalidOperationException or Win32Exception)
                {
                    if (Group(process.Id) != -1 || Marshal.GetLastPInvokeError() != NoSuchProcess) throw;
                }
            }
        }
        finally { foreach (Process process in processes) process.Dispose(); }
        return total;
    }

    private static void KillGroup(int group)
    {
        // Never signal the caller's group, PID 1, or every process accessible to this user.
        if (group <= 1 || (group != Environment.ProcessId && group == Group(0)))
            throw new InvalidOperationException("Refusing unsafe process-group termination.");
        if (Kill(-group, 9) != 0 && Marshal.GetLastPInvokeError() != NoSuchProcess)
            throw new Win32Exception(Marshal.GetLastPInvokeError(), "kill process group");
    }

    [LibraryImport("libc", EntryPoint = "ioctl", SetLastError = true)]
    private static partial int Ioctl(int descriptor, nuint request);
    [LibraryImport("libc", EntryPoint = "setpgid", SetLastError = true)]
    private static partial int SetGroup(int process, int group);
    [LibraryImport("libc", EntryPoint = "getpgid", SetLastError = true)]
    private static partial int Group(int process);
    [LibraryImport("libc", EntryPoint = "getppid")]
    private static partial int Parent();
    [LibraryImport("libc", EntryPoint = "kill", SetLastError = true)]
    private static partial int Kill(int process, int signal);
}
