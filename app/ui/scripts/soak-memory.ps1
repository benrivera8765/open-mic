# SPDX-License-Identifier: GPL-3.0-or-later
# Memory sampler for real-app-soak.mjs: every <Interval> seconds, one CSV line per process
# in the tree under <RootPid> (Open Mic.exe, its WebView2 processes, the Studio): working
# set, peak working set, private bytes (commit) and peak private bytes, from Win32_Process.
#
#   powershell -NoProfile -File soak-memory.ps1 <RootPid> <out.csv> <seconds> [interval]
param([int]$RootPid, [string]$Out, [int]$Seconds, [int]$Interval = 2)

"t_s,pid,ppid,name,working_set,peak_working_set,private_bytes,peak_private_bytes" | Out-File -Encoding ascii $Out
$t0 = Get-Date
$end = $t0.AddSeconds($Seconds)
while ((Get-Date) -lt $end) {
    $all = @(Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name, WorkingSetSize, PeakWorkingSetSize, PrivatePageCount, PeakPageFileUsage)
    $byId = @{}
    $kids = @{}
    foreach ($p in $all) {
        $byId[[int]$p.ProcessId] = $p
        $pp = [int]$p.ParentProcessId
        if (-not $kids.ContainsKey($pp)) { $kids[$pp] = New-Object System.Collections.ArrayList }
        [void]$kids[$pp].Add($p)
    }
    if (-not $byId.ContainsKey($RootPid)) { break }
    $t = ((Get-Date) - $t0).TotalSeconds
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue($byId[$RootPid])
    $seen = @{}
    $lines = New-Object System.Collections.ArrayList
    while ($queue.Count -gt 0) {
        $p = $queue.Dequeue()
        $id = [int]$p.ProcessId
        if ($seen.ContainsKey($id)) { continue }
        $seen[$id] = $true
        # PeakWorkingSetSize and PeakPageFileUsage are in KB; the others in bytes.
        [void]$lines.Add(("{0:F1},{1},{2},{3},{4},{5},{6},{7}" -f $t, $id, $p.ParentProcessId, $p.Name, $p.WorkingSetSize, ([int64]$p.PeakWorkingSetSize * 1024), $p.PrivatePageCount, ([int64]$p.PeakPageFileUsage * 1024)))
        if ($kids.ContainsKey($id)) { foreach ($c in $kids[$id]) { if ([int]$c.ProcessId -ne $id) { $queue.Enqueue($c) } } }
    }
    $lines | Out-File -Append -Encoding ascii $Out
    Start-Sleep -Seconds $Interval
}
