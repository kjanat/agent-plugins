[CmdletBinding()]
param(
	[Parameter(ValueFromRemainingArguments = $true)]
	[string[]] $ServerArguments = @()
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

try {
	if (-not [Environment]::Is64BitProcess) {
		throw 'The memory guard requires 64-bit Windows PowerShell on Windows 10 or newer.'
	}

	$server = $env:RUST_ANALYZER_EXECUTABLE
	if ([string]::IsNullOrWhiteSpace($server)) { $server = 'rust-analyzer.exe' }
	$memoryLimit = $env:RUST_ANALYZER_MEMORY_LIMIT_MIB
	if ([string]::IsNullOrWhiteSpace($memoryLimit)) { $memoryLimit = '6144' }

	if ($server -match '^(?:[A-Za-z]:[\\/]|[\\/]{2})') {
		$executable = (Get-Item -LiteralPath $server).FullName
	}
	else {
		if ($server.IndexOfAny([char[]]'*?[]\/:') -ge 0) {
			throw 'Use an executable name on PATH or an absolute path; relative paths and wildcards are not supported.'
		}
		$executable = @(Get-Command -Name $server -CommandType Application)[0].Source
	}
	if ([IO.Path]::GetExtension($executable) -ine '.exe') {
		throw 'rust-analyzer must be a native .exe, not a shell script or batch wrapper.'
	}

	# Compile in this process: a PowerShell native-command pipeline can transform LSP bytes.
	# Native handle inheritance below passes the original pipes directly to rust-analyzer.
	Add-Type -Path (Join-Path $PSScriptRoot '../native/Guard.cs') -ErrorAction Stop
	$guardArguments = @('--limit-mib', $memoryLimit, '--', $executable) + $ServerArguments
	$exitCode = [AgentPlugins.RustAnalyzer.RustAnalyzerGuard]::Main($guardArguments)
	exit $exitCode
}
catch {
	[Console]::Error.WriteLine('rust-guard: refusing unguarded launch: ' + $_.Exception.Message)
	exit 125
}
