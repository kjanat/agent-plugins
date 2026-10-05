using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

namespace AgentPlugins.RustAnalyzer
{
    internal static partial class WindowsGuard
    {
        private const uint BudgetExitCode = 137;

        private static void Require(bool success, string operation)
        {
            if (!success) throw new Win32Exception(Marshal.GetLastWin32Error(), operation);
        }

        private static string Quote(string value)
        {
            var result = new StringBuilder("\"");
            int slashes = 0;
            foreach (char character in value)
            {
                if (character == '\\') { slashes++; continue; }
                if (character == '"') result.Append('\\', slashes * 2 + 1);
                else result.Append('\\', slashes);
                result.Append(character);
                slashes = 0;
            }
            return result.Append('\\', slashes * 2).Append('"').ToString();
        }

        internal static unsafe int Run(string executable, string[] arguments, ulong limitMiB)
        {
            ulong limitBytes = checked(limitMiB * 1024 * 1024);
            var commandLine = new StringBuilder(Quote(executable));
            foreach (string argument in arguments) commandLine.Append(' ').Append(Quote(argument));

            IntPtr job = IntPtr.Zero;
            IntPtr attributes = IntPtr.Zero;
            IntPtr inheritedList = IntPtr.Zero;
            IntPtr jobList = IntPtr.Zero;
            bool attributesInitialized = false;
            var handles = new IntPtr[3];
            var process = new Native.ProcessInformation();
            try
            {
                job = Native.CreateJobObject(IntPtr.Zero, null);
                Require(job != IntPtr.Zero, "CreateJobObject");
                var limits = new Native.ExtendedLimits();
                // Deny excess commit across the entire LSP tree; never let children escape it.
                limits.Basic.LimitFlags = 0x200 | 0x2000; // JOB_MEMORY | KILL_ON_JOB_CLOSE
                limits.JobMemoryLimit = new UIntPtr(limitBytes);
                Require(Native.SetInformationJobObject(job, 9, ref limits,
                    (uint)Marshal.SizeOf<Native.ExtendedLimits>()), "SetInformationJobObject");

                IntPtr self = Native.GetCurrentProcess();
                for (int i = 0; i < handles.Length; i++)
                    Require(Native.DuplicateHandle(self, Native.GetStdHandle(-10 - i), self,
                        out handles[i], 0, true, 2), "Duplicate standard handle");
                inheritedList = Marshal.AllocHGlobal(IntPtr.Size * 3);
                Marshal.Copy(handles, 0, inheritedList, handles.Length);
                jobList = Marshal.AllocHGlobal(IntPtr.Size);
                Marshal.WriteIntPtr(jobList, job);
                IntPtr size = IntPtr.Zero;
                Native.InitializeProcThreadAttributeList(IntPtr.Zero, 2, 0, ref size);
                if (size == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error(), "Attribute list size");
                attributes = Marshal.AllocHGlobal(size);
                Require(Native.InitializeProcThreadAttributeList(attributes, 2, 0, ref size), "Initialize attributes");
                attributesInitialized = true;
                Require(Native.UpdateProcThreadAttribute(attributes, 0, new IntPtr(0x20002),
                    inheritedList, new IntPtr(IntPtr.Size * 3), IntPtr.Zero, IntPtr.Zero), "Restrict inherited handles");
                // Atomic job assignment prevents a launcher crash between creation and assignment
                // from leaving an unbounded or suspended child behind. Requires Windows 10+.
                Require(Native.UpdateProcThreadAttribute(attributes, 0, new IntPtr(0x2000D),
                    jobList, new IntPtr(IntPtr.Size), IntPtr.Zero, IntPtr.Zero), "Assign job at creation");
                var startup = new Native.StartupInformationEx();
                startup.Startup.Size = Marshal.SizeOf<Native.StartupInformationEx>();
                startup.Startup.Flags = 0x100; // STARTF_USESTDHANDLES
                startup.Startup.Input = handles[0];
                startup.Startup.Output = handles[1];
                startup.Startup.Error = handles[2];
                startup.Attributes = attributes;
                // CreateProcessW may modify its UTF-16 command-line buffer.
                char[] buffer = (commandLine.ToString() + "\0").ToCharArray();
                fixed (char* command = buffer)
                    Require(Native.CreateProcess(executable, command, IntPtr.Zero, IntPtr.Zero,
                        true, 0x08080000, IntPtr.Zero, null, ref startup, out process), "Create guarded process");

                while (true)
                {
                    uint wait = Native.WaitForSingleObject(process.Process, 100);
                    if (wait == 0)
                    {
                        uint exitCode;
                        Require(Native.GetExitCodeProcess(process.Process, out exitCode), "Get exit code");
                        return unchecked((int)exitCode);
                    }
                    Require(wait == 258, "Wait for process");
                    Native.ExtendedLimits usage;
                    Require(Native.QueryInformationJobObject(job, 9, out usage,
                        (uint)Marshal.SizeOf<Native.ExtendedLimits>(), IntPtr.Zero), "Read job memory");
                    // A one-shot high-water trigger leaves room below the kernel's hard ceiling.
                    // The hard cap remains effective between polls, including fast allocations.
                    if (usage.PeakJobMemoryUsed.ToUInt64() >= limitBytes * 95 / 100)
                    {
                        Require(Native.TerminateJobObject(job, BudgetExitCode), "Stop LSP at memory budget");
                        Console.Error.WriteLine("rust-guard: LSP stopped near its {0} MiB memory ceiling; Claude is outside this job.", limitMiB);
                        return (int)BudgetExitCode;
                    }
                }
            }
            finally
            {
                // Also removes compiler/proc-macro children after normal LSP exit or guard errors.
                Close(job);
                Close(process.Thread);
                Close(process.Process);
                foreach (IntPtr handle in handles) Close(handle);
                if (attributesInitialized) Native.DeleteProcThreadAttributeList(attributes);
                if (attributes != IntPtr.Zero) Marshal.FreeHGlobal(attributes);
                if (inheritedList != IntPtr.Zero) Marshal.FreeHGlobal(inheritedList);
                if (jobList != IntPtr.Zero) Marshal.FreeHGlobal(jobList);
            }
        }

        private static void Close(IntPtr handle)
        {
            if (handle != IntPtr.Zero && !Native.CloseHandle(handle))
                Console.Error.WriteLine("rust-guard: CloseHandle failed: " + Marshal.GetLastWin32Error());
        }

        private static partial class Native
        {
            [StructLayout(LayoutKind.Sequential)]
            internal struct BasicLimits
            {
                internal long ProcessTime, JobTime;
                internal uint LimitFlags;
                internal UIntPtr MinimumWorkingSet, MaximumWorkingSet;
                internal uint ActiveProcesses;
                internal UIntPtr Affinity;
                internal uint PriorityClass, SchedulingClass;
            }
            [StructLayout(LayoutKind.Sequential)]
            internal struct IoCounters { internal ulong ReadOperations, WriteOperations, OtherOperations, ReadBytes, WriteBytes, OtherBytes; }
            [StructLayout(LayoutKind.Sequential)]
            internal struct ExtendedLimits
            {
                internal BasicLimits Basic;
                internal IoCounters Io;
                internal UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemoryUsed, PeakJobMemoryUsed;
            }
            [StructLayout(LayoutKind.Sequential)]
            internal struct StartupInformation
            {
                internal int Size;
                internal IntPtr Reserved, Desktop, Title;
                internal uint X, Y, XSize, YSize, XCountChars, YCountChars, FillAttribute, Flags;
                internal ushort ShowWindow, ReservedSize;
                internal IntPtr ReservedBytes, Input, Output, Error;
            }
            [StructLayout(LayoutKind.Sequential)]
            internal struct StartupInformationEx { internal StartupInformation Startup; internal IntPtr Attributes; }
            [StructLayout(LayoutKind.Sequential)]
            internal struct ProcessInformation { internal IntPtr Process, Thread; internal uint ProcessId, ThreadId; }

            [LibraryImport("kernel32.dll", EntryPoint = "CreateJobObjectW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
            internal static partial IntPtr CreateJobObject(IntPtr security, string? name);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool SetInformationJobObject(IntPtr job, int kind, ref ExtendedLimits value, uint length);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool QueryInformationJobObject(IntPtr job, int kind, out ExtendedLimits value, uint length, IntPtr returned);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool TerminateJobObject(IntPtr job, uint code);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool CloseHandle(IntPtr handle);
            [LibraryImport("kernel32.dll")]
            internal static partial IntPtr GetCurrentProcess();
            [LibraryImport("kernel32.dll", SetLastError = true)]
            internal static partial IntPtr GetStdHandle(int kind);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool DuplicateHandle(IntPtr sourceProcess, IntPtr source, IntPtr destinationProcess,
                out IntPtr destination, uint access, [MarshalAs(UnmanagedType.Bool)] bool inherit, uint options);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool InitializeProcThreadAttributeList(IntPtr list, int count, uint flags, ref IntPtr size);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static partial bool UpdateProcThreadAttribute(IntPtr list, uint flags, IntPtr attribute,
                IntPtr value, IntPtr size, IntPtr previous, IntPtr returned);
            [LibraryImport("kernel32.dll")]
            internal static partial void DeleteProcThreadAttributeList(IntPtr list);
            [LibraryImport("kernel32.dll", EntryPoint = "CreateProcessW", StringMarshalling = StringMarshalling.Utf16, SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]
            internal static unsafe partial bool CreateProcess(string executable, char* commandLine, IntPtr processSecurity,
                IntPtr threadSecurity, [MarshalAs(UnmanagedType.Bool)] bool inheritHandles, uint flags, IntPtr environment, string? directory,
                ref StartupInformationEx startup, out ProcessInformation process);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            internal static partial uint WaitForSingleObject(IntPtr handle, uint milliseconds);
            [LibraryImport("kernel32.dll", SetLastError = true)]
            [return: MarshalAs(UnmanagedType.Bool)]

            internal static partial bool GetExitCodeProcess(IntPtr process, out uint code);
        }
    }
}
