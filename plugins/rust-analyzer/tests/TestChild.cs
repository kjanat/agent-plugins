using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace AgentPlugins.RustAnalyzer.Tests;

internal static partial class TestChild
{
    [LibraryImport("kernel32.dll", SetLastError = true)]
    private static partial IntPtr VirtualAlloc(IntPtr address, UIntPtr size, uint allocation, uint protection);

    private static Process Child(string mode)
    {
        var start = new ProcessStartInfo(Environment.ProcessPath
            ?? throw new InvalidOperationException("Missing helper executable"), mode)
        {
            UseShellExecute = false,
            CreateNoWindow = true
        };
        return Process.Start(start) ?? throw new InvalidOperationException("Child did not start");
    }

    public static int Main(string[] args)
    {
        switch (args[0])
        {
            case "echo":
                Console.Error.Write("stderr-only");
                Console.OpenStandardInput().CopyTo(Console.OpenStandardOutput());
                return 7;
            case "args":
                for (int i = 1; i < args.Length; i++)
                    Console.WriteLine(Convert.ToBase64String(Encoding.UTF8.GetBytes(args[i])));
                return 0;
            case "deny":
                bool denied = VirtualAlloc(IntPtr.Zero, new UIntPtr(512UL * 1024 * 1024), 0x3000, 4) == IntPtr.Zero;
                Console.WriteLine(denied ? "allocation-denied" : "allocation-granted");
                return denied ? 42 : 43;
            case "tree":
                using (Process child = Child("allocate"))
                {
                    Console.WriteLine(Environment.ProcessId + " " + child.Id);
                    Thread.Sleep(4000);
                }
                return 0;
            case "allocate":
                var allocations = new List<IntPtr>();
                try
                {
                    for (int i = 0; i < 160; i++)
                    {
                        IntPtr block = Marshal.AllocHGlobal(2 * 1024 * 1024);
                        allocations.Add(block);
                        // Touch every page: Unix budgets resident bytes, not virtual reservations.
                        for (int offset = 0; offset < 2 * 1024 * 1024; offset += 4096)
                            Marshal.WriteByte(block, offset, 1);
                        Thread.Sleep(15);
                    }
                    Thread.Sleep(500);
                }
                finally { foreach (IntPtr block in allocations) Marshal.FreeHGlobal(block); }
                return 0;
            case "hold":
                using (Process child = Child("stay"))
                {
                    Console.WriteLine(Environment.ProcessId + " " + child.Id);
                    Thread.Sleep(4000);
                }
                return 0;
            case "orphan":
                using (Process child = Child("stay")) Console.WriteLine(child.Id);
                return 0;
            case "stay":
                Thread.Sleep(4000);
                return 0;
            default: return 99;
        }
    }
}
