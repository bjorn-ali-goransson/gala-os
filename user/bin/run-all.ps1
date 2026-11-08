<#
.SYNOPSIS
Runs gala-os's test programs on Windows and writes a report for
`cargo xtask compare`.

.DESCRIPTION
Runs every *.exe next to this script (PowerShell 5 or later, no admin
rights needed, except for schannel_test, which is skipped without them),
captures each program's output and exit status, and writes
them as JSON in the schema of `cargo xtask report` (see
xtask/src/report.rs). Programs report through their exit status: 0, the
number of the failing check, or an NTSTATUS if they crash.

The results are meant for the pinned build (Windows 11 24H2, 26100): the
programs use its raw system call numbers.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File .\run-all.ps1 -Out windows.json
#>
param(
    [string]$Out = (Join-Path (Get-Location) "windows.json"),
    [int]$TimeoutSeconds = 60
)
Set-StrictMode -Version 2
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
# `-Out windows.json`, the documented form, has no directory part, and
# Split-Path would give an empty string for it: resolve it against the
# current directory, so the files written beside the report have a path.
if (-not [IO.Path]::IsPathRooted($Out)) { $Out = Join-Path (Get-Location).Path $Out }
$outDir = Split-Path -Parent $Out

# A crashing program (crash.exe crashes on purpose) must not open a Windows
# Error Reporting dialog; the error mode is inherited by child processes.
Add-Type -Namespace Gala -Name Native -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern uint SetErrorMode(uint mode);
'@
# SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX
[void][Gala.Native]::SetErrorMode(0x0001 -bor 0x0002 -bor 0x8000)

$runStart = Get-Date
# Kills what a program left running. Kill() reaches the program itself, not
# the children it started, and those keep its standard output open (below).
# Only processes started after this run began, so that a process which has
# since taken a dead one's id is left alone.
function Stop-Descendants([int]$parent, [datetime]$since) {
    $all = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue)
    foreach ($p in @($all | Where-Object {
        $_.ParentProcessId -eq $parent -and $null -ne $_.CreationDate -and $_.CreationDate -ge $since
    })) {
        Stop-Descendants $p.ProcessId $since
        Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

# Command lines that scenarios pass (gala: `args=` in xtask/src/main.rs).
# nginx.exe would serve until stopped: -v prints its version and exits.
$arguments = @{ "args_test" = 'alpha "beta gamma"'; "wow64_crt_test" = 'one "two three"'; "go_http" = 'self'; "handover_net_test" = 'self'; "nginx" = '-v'; "busybox" = 'sh -c "echo hello | cat; echo pipe=$?; echo abc | sed s/b/X/; echo count=$(printf ''x\ny\nx\n'' | grep -c x); exit 3"' }
# smp_test takes an expected processor count (cpus=N); none on Windows.

$results = @()

# schannel_test talks TLS to Nginx on 127.0.0.1:8443-8447 (its scenario
# starts Nginx with `start=`): Nginx runs from a prefix made in TEMP with
# ..\schannel_test\nginx.conf and its certificates, and the test CA must be
# a trusted root of the machine, which only an administrator can make it
# (then it is removed again). Without that the test is skipped.
$schannelSkip = $null
$schannelNginx = $null
$schannelCa = $null
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator)
$schannelDir = Join-Path $here "..\schannel_test"
if (-not (Test-Path (Join-Path $here "schannel_test.exe"))) {
    $schannelSkip = "no schannel_test.exe"
} elseif (-not $admin) {
    $schannelSkip = "needs an administrator (the test CA goes into LocalMachine\Root)"
} elseif (-not (Test-Path (Join-Path $here "nginx.exe"))) {
    $schannelSkip = "no nginx.exe"
} else {
    $prefix = Join-Path $env:TEMP "gala-schannel-nginx"
    foreach ($d in @("conf", "html", "logs", "temp")) {
        New-Item -ItemType Directory -Force -Path (Join-Path $prefix $d) | Out-Null
    }
    Copy-Item (Join-Path $schannelDir "nginx.conf") (Join-Path $prefix "conf")
    Copy-Item (Join-Path $schannelDir "certs\*.crt") (Join-Path $prefix "conf")
    Copy-Item (Join-Path $schannelDir "certs\*.key") (Join-Path $prefix "conf")
    Copy-Item (Join-Path $here "..\nginx\html\index.html") (Join-Path $prefix "html")
    # The test CA's CRL at the distribution point its revocation
    # certificates name (http://127.0.0.1:8080/ca.crl): Windows fetches it.
    Copy-Item (Join-Path $schannelDir "certs\ca.crl") (Join-Path $prefix "html")
    $schannelCa = Import-Certificate -FilePath (Join-Path $schannelDir "certs\ca.cer") `
        -CertStoreLocation Cert:\LocalMachine\Root
    $schannelNginx = Start-Process -FilePath (Join-Path $here "nginx.exe") -PassThru -WindowStyle Hidden `
        -ArgumentList @("-p", "$prefix\", "-c", "conf/nginx.conf")
}

foreach ($exe in Get-ChildItem -Path $here -Filter *.exe | Sort-Object Name) {
    $name = [IO.Path]::GetFileNameWithoutExtension($exe.Name)
    # Driven by flaui_test (a .NET program: docs/WORKFLOW.md, "FlaUI");
    # alone it waits for its window to be closed.
    if ($name -eq "flaui_target") { continue }
    if ($name -eq "schannel_test" -and $null -ne $schannelSkip) {
        $results += [ordered]@{ program = $name; exit = $null; line = ""; skipped = $schannelSkip }
        Write-Host ("{0,-18} skipped: {1}" -f $name, $schannelSkip)
        continue
    }
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe.FullName
    # import_test and loader_test load testdll*.dll from the executable's
    # directory.
    $psi.WorkingDirectory = $here
    if ($arguments.ContainsKey($name)) { $psi.Arguments = $arguments[$name] }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.CreateNoWindow = $true
    # Antivirus can refuse to start a program, and may have taken the file
    # between Get-ChildItem and here: the test binaries are unsigned and
    # freshly built, which Microsoft Defender's machine-learning heuristic
    # takes for malware (bench.exe as Trojan:Win32/Bearfoos.A!ml). Record
    # why and go on; one program the machine will not run must not cost
    # every other program's result.
    $process = $null
    try {
        $process = [System.Diagnostics.Process]::Start($psi)
    } catch {
        # A .NET method's exception reaches us inside a
        # MethodInvocationException, so look for the Win32 one.
        $ex = $_.Exception
        while ($ex -isnot [ComponentModel.Win32Exception] -and $null -ne $ex.InnerException) {
            $ex = $ex.InnerException
        }
        $errno = if ($ex -is [ComponentModel.Win32Exception]) { $ex.NativeErrorCode } else { 0 }
        $why = switch ($errno) {
            225 { "antivirus blocked it (ERROR_VIRUS_INFECTED)" }      # 0xe1
            226 { "antivirus deleted it (ERROR_VIRUS_DELETED)" }       # 0xe2
            default { "did not start: " + $ex.Message.Trim() }
        }
        $results += [ordered]@{ program = $name; exit = $null; line = ""; skipped = $why }
        Write-Host ("{0,-18} skipped: {1}" -f $name, $why)
    }
    if ($null -eq $process) { continue }
    $stdout = $process.StandardOutput.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        $process.Kill()
        $process.WaitForExit()
        Stop-Descendants $process.Id $runStart
        $results += [ordered]@{
            program = $name; exit = $null; line = ""; skipped = "timed out after $TimeoutSeconds s"
        }
        Write-Host ("{0,-18} timed out" -f $name)
        continue
    }
    $process.WaitForExit()
    # ReadToEndAsync ends at EOF, and that needs every handle on the write
    # end of the pipe closed: a program that leaves a child of its own
    # running keeps one open (quota_test creates its children suspended and
    # leaks them when a check fails before it resumes them), and waiting
    # for the text would then hang for good, past the timeout above, which
    # waited only for the program itself. So bound this wait too, clear the
    # children out, and go on with what arrived.
    if (-not $stdout.Wait($TimeoutSeconds * 1000)) {
        Write-Host ("{0,-18} its output is held open by a child it left running" -f $name)
        Stop-Descendants $process.Id $runStart
        [void]$stdout.Wait(5000)
    }
    $lost = $stdout.Status -ne [Threading.Tasks.TaskStatus]::RanToCompletion
    $text = if ($lost) { "" } else { $stdout.Result }
    # ExitCode is signed; report the 32-bit value as unsigned (NTSTATUS).
    $code = [BitConverter]::ToUInt32([BitConverter]::GetBytes([int32]$process.ExitCode), 0)
    $lines = @($text -split "`r?`n")
    $line = $lines | Where-Object { $_ -like "${name}: *" } | Select-Object -First 1
    if ($null -eq $line) { $line = $lines | Select-Object -First 1 }
    if ($null -eq $line) { $line = "" }
    if ($lost) { $line = "(output lost: a child it left running held the pipe open)" }
    $entry = [ordered]@{ program = $name; exit = $code; line = $line }
    # A crash (an NTSTATUS error as the exit code: 0xC0000005 and the like)
    # leaves only its code here; Windows Error Reporting logs where it
    # happened (Application log, event 1000, readable without
    # administrator rights): the faulting module, exception code and
    # offset, which `cargo xtask symbolize`-style lookups map to a function
    # through the build's PDB. WER may take a moment to write it.
    if ($code -ge 0xC0000000) {
        $pidHex = "0x{0:x}" -f $process.Id
        $fault = $null
        foreach ($try in 1..6) {
            $fault = Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = 1000; StartTime = $runStart } `
                -ErrorAction SilentlyContinue |
                Where-Object { $_.Message -like "*$($exe.Name)*" -and $_.Message -like "*$pidHex*" } |
                Select-Object -First 1
            if ($null -ne $fault) { break }
            Start-Sleep -Milliseconds 500
        }
        if ($null -ne $fault) {
            $m = $fault.Message
            $module = if ($m -match 'Faulting module name: ([^,\r\n]+)') { $Matches[1].Trim() } else { "?" }
            $exception = if ($m -match 'Exception code: (0x[0-9a-fA-F]+)') { $Matches[1] } else { "?" }
            $offset = if ($m -match 'Fault offset: (0x[0-9a-fA-F]+)') { $Matches[1] } else { "?" }
            $entry.fault = "$module+$offset ($exception)"
            Write-Host ("{0,-18} crashed in {1}" -f $name, $entry.fault)
        } else {
            $entry.fault = "no Application Error event found"
        }
    }
    $results += $entry
    # The benchmarks' numbers, for `cargo xtask bench <this file>`.
    if ($name -eq "bench") {
        $text | Set-Content -Path (Join-Path $outDir "bench-windows.txt") -Encoding UTF8
    }
    Write-Host ("{0,-18} {1:x8}  {2}" -f $name, $code, $line)
}

if ($null -ne $schannelNginx) { Stop-Process -Id $schannelNginx.Id -Force -ErrorAction SilentlyContinue }
if ($null -ne $schannelCa) { $schannelCa | Remove-Item -ErrorAction SilentlyContinue }

# A quarantined program is gone from the directory, so the loop above never
# saw it and the report would just lack it, with nothing to say why.
# Microsoft Defender's own history names the file and the threat; this is
# best-effort (another antivirus, or no Defender module, leaves it out).
try {
    $threatNames = @{}
    foreach ($t in Get-MpThreat -ErrorAction Stop) {
        $threatNames[[string]$t.ThreatID] = $t.ThreatName
    }
    foreach ($d in Get-MpThreatDetection -ErrorAction Stop) {
        foreach ($res in @($d.Resources)) {
            # Resources read "file:_C:\path\to\it.exe".
            if ($res -notlike "file:_*") { continue }
            $path = $res.Substring(6)
            if ([IO.Path]::GetDirectoryName($path) -ne $here) { continue }
            if ([IO.Path]::GetExtension($path) -ne ".exe") { continue }
            if (Test-Path $path) { continue }
            $gone = [IO.Path]::GetFileNameWithoutExtension($path)
            if ($results | Where-Object { $_.program -eq $gone }) { continue }
            $threat = $threatNames[[string]$d.ThreatID]
            if (-not $threat) { $threat = "threat $($d.ThreatID)" }
            $why = "quarantined by Microsoft Defender as $threat"
            $results += [ordered]@{ program = $gone; exit = $null; line = ""; skipped = $why }
            Write-Host ("{0,-18} skipped: {1}" -f $gone, $why)
        }
    }
} catch {
    Write-Host ("note: no antivirus detections read ({0})" -f $_.Exception.Message.Trim())
}

# Scripts run by busybox sh (the busybox_files, busybox_script and tcc
# scenarios): their full output goes to <name>-windows.txt next to the
# report, to compare with the scenario's expected lines. tcc.sh unpacks
# tcc.tar (user/tcc/build.sh), builds TinyCC's runtime with it, compiles
# hello.c and runs it; self.sh then has tcc compile itself (tcc2.exe), and
# tcc2 compile hello.c and msvcrt_test's main.c; make.sh builds
# user/tcc/make/ with busybox's make and tcc; lua.sh builds Lua from
# source with them and runs user/lua/test.lua; sqlite.sh builds SQLite
# with tcc and runs user/sqlite/test.sql and reopen.sql.
$scripts = @(
    @{ name = "busybox_files"; path = "..\busybox\files.sh"; args = @() },
    @{ name = "busybox_script"; path = "..\busybox\script.sh"; args = @() },
    @{ name = "tcc"; path = "..\tcc\tcc.sh"; args = @((Join-Path $here "tcc.tar")) },
    @{ name = "tcc_make"; path = "..\tcc\make.sh"; args = @((Join-Path $here "tcc.tar")) },
    @{ name = "lua"; path = "..\lua\lua.sh"; args = @((Join-Path $here "tcc.tar"), (Join-Path $here "lua.tar")) },
    @{ name = "sqlite"; path = "..\sqlite\sqlite.sh"; args = @((Join-Path $here "tcc.tar"), (Join-Path $here "sqlite-amalgamation.zip")) },
    @{ name = "tcc_self"; path = "..\tcc\self.sh"; args = @((Join-Path $here "tcc.tar"), (Join-Path $here "..\msvcrt_test\main.c")) }
)
foreach ($s in $scripts) {
    $path = Join-Path $here $s.path
    if (-not (Test-Path $path) -or -not (Test-Path (Join-Path $here "busybox.exe"))) { continue }
    $text = & (Join-Path $here "busybox.exe") sh $path @($s.args) 2>&1 | Out-String
    $code = $LASTEXITCODE
    $text | Set-Content -Path (Join-Path $outDir "$($s.name)-windows.txt") -Encoding UTF8
    $last = @($text -split "`r?`n" | Where-Object { $_ -ne "" }) | Select-Object -Last 1
    $results += [ordered]@{ program = $s.name; exit = $code; line = $last }
    Write-Host ("{0,-18} {1:x8}  {2}" -f $s.name, $code, $last)
}

$os = Get-CimInstance Win32_OperatingSystem
$report = [ordered]@{ platform = "windows " + $os.Version; results = $results }
$report | ConvertTo-Json -Depth 4 | Set-Content -Path $Out -Encoding UTF8
Write-Host "wrote $Out"
