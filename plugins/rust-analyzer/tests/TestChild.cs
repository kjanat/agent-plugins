using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

internal static class TestChild
{
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr VirtualAlloc(IntPtr address, UIntPtr size, uint allocation, uint protection);

    private static Process Child(string mode)
    {
        using (Process current = Process.GetCurrentProcess())
        {
            var start = new ProcessStartInfo(current.MainModule.FileName, mode)
            {
                UseShellExecute = false,
                CreateNoWindow = true
            };
            return Process.Start(start);
        }
    }

    private static int CurrentProcessId()
    {
        using (Process current = Process.GetCurrentProcess()) return current.Id;
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
                return VirtualAlloc(IntPtr.Zero, new UIntPtr(128UL * 1024 * 1024), 0x3000, 4) == IntPtr.Zero ? 42 : 43;
            case "tree":
                using (Process child = Child("allocate"))
                {
                    Console.WriteLine(CurrentProcessId() + " " + child.Id);
                    Thread.Sleep(4000);
                }
                return 0;
            case "allocate":
                for (int i = 0; i < 64; i++)
                {
                    if (VirtualAlloc(IntPtr.Zero, new UIntPtr(2UL * 1024 * 1024), 0x3000, 4) == IntPtr.Zero)
                        return 44;
                    Thread.Sleep(20);
                }
                return 0;
            case "hold":
                using (Process child = Child("stay"))
                {
                    Console.WriteLine(CurrentProcessId() + " " + child.Id);
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
