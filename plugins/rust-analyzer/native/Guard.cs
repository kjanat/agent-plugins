using System.Globalization;
using System.Diagnostics;
using System.Runtime.InteropServices;

namespace AgentPlugins.RustAnalyzer;

internal static class RustAnalyzerGuard
{
    internal const int FailureExitCode = 125;
    internal const int BudgetExitCode = 137;

    public static int Main(string[] arguments)
    {
        try
        {
            if (RuntimeInformation.ProcessArchitecture is not (Architecture.X64 or Architecture.Arm64))
                throw new PlatformNotSupportedException("An x64 or ARM64 .NET runtime is required.");
            if (arguments.Length > 0 && arguments[0] == "--guard-unix-child")
                return UnixGuard.RunChild(arguments);
            string budget = Environment.GetEnvironmentVariable("RUST_ANALYZER_MEMORY_LIMIT_MIB") ?? "6144";
            if (!ulong.TryParse(budget, NumberStyles.None, CultureInfo.InvariantCulture, out ulong limit)
                || limit < 32 || limit > 1048576)
                throw new ArgumentException("memory budget must be whole MiB between 32 and 1048576");
            string executable = Resolve(Environment.GetEnvironmentVariable("RUST_ANALYZER_EXECUTABLE") ?? "rust-analyzer");
            if (OperatingSystem.IsWindows()) return WindowsGuard.Run(executable, arguments, limit);
            if (OperatingSystem.IsLinux() || OperatingSystem.IsMacOS()) return UnixGuard.Run(executable, arguments, limit);
            throw new PlatformNotSupportedException("Supported hosts: Windows, Linux and macOS.");
        }
        catch (Exception error)
        {
            Console.Error.WriteLine("rust-guard: refusing unguarded launch: " + error.Message);
            return FailureExitCode;
        }
    }

    private static string Resolve(string value)
    {
        if (Path.IsPathFullyQualified(value)) return Validate(value);
        if (string.IsNullOrWhiteSpace(value) || value.IndexOfAny(['/', '\\', ':']) >= 0)
            throw new ArgumentException("relative paths are not supported; use an absolute executable or a PATH name");
        string name = OperatingSystem.IsWindows() && !value.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)
            ? value + ".exe" : value;
        foreach (string directory in (Environment.GetEnvironmentVariable("PATH") ?? "").Split(Path.PathSeparator))
        {
            // Do not resolve from cwd through empty or relative PATH entries.
            if (!Path.IsPathFullyQualified(directory)) continue;
            string candidate = Path.Combine(directory, name);
            if (File.Exists(candidate)) return Validate(candidate);
        }
        throw new FileNotFoundException("server not found on PATH: " + value);
    }

    private static string Validate(string path)
    {
        if (!File.Exists(path)) throw new FileNotFoundException("server not found: " + path);
        if (OperatingSystem.IsWindows() && !path.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("Windows server must be a native .exe");
        return path;
    }

    internal static Process Start(ProcessStartInfo start) => Process.Start(start)
        ?? throw new InvalidOperationException("Could not start guarded process.");
}
