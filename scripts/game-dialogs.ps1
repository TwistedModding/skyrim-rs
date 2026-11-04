# A plugin that refuses the runtime may stop the game with a message box (class #32770) until someone clicks it, and so
# does the script extender after refusing a plugin (Exit game? Yes/No). Close every such box of the game, answering No
# where there is one, keeping its title and text in $OutDir\dialogs.txt, so an unattended run goes on. Dot-sourced.
Add-Type -Namespace DevBench -Name Win32 -MemberDefinition @'
public delegate bool EnumProc(System.IntPtr hwnd, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool EnumChildWindows(System.IntPtr parent, EnumProc proc, System.IntPtr lparam);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr hwnd, out uint pid);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetClassName(System.IntPtr hwnd, System.Text.StringBuilder name, int max);
[System.Runtime.InteropServices.DllImport("user32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode)] public static extern int GetWindowText(System.IntPtr hwnd, System.Text.StringBuilder text, int max);
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool PostMessage(System.IntPtr hwnd, uint msg, System.IntPtr w, System.IntPtr l);
'@
function Close-Dialogs {
    $ids = @(Get-Process -Name SkyrimSE -ErrorAction SilentlyContinue | ForEach-Object { [uint32]$_.Id })
    $found = New-Object System.Collections.ArrayList
    [void][DevBench.Win32]::EnumWindows({ param($hwnd, $l)
        $procId = [uint32]0
        [void][DevBench.Win32]::GetWindowThreadProcessId($hwnd, [ref]$procId)
        $class = New-Object System.Text.StringBuilder 64
        [void][DevBench.Win32]::GetClassName($hwnd, $class, 64)
        if ($procId -in $ids -and $class.ToString() -eq "#32770") { [void]$found.Add($hwnd) }
        $true }, [IntPtr]::Zero)
    foreach ($hwnd in $found) {
        $texts = New-Object System.Collections.ArrayList
        $title = New-Object System.Text.StringBuilder 512
        [void][DevBench.Win32]::GetWindowText($hwnd, $title, 512)
        $script:no = $null
        [void][DevBench.Win32]::EnumChildWindows($hwnd, { param($child, $l)
            $t = New-Object System.Text.StringBuilder 2048
            [void][DevBench.Win32]::GetWindowText($child, $t, 2048)
            if ($t.Length) { [void]$texts.Add($t.ToString()) }
            if ($t.ToString() -eq "&No") { $script:no = $child }
            $true }, [IntPtr]::Zero)
        $line = "closed a message box titled '$title': $(($texts -join ' | ') -replace '\s+', ' ')"
        Write-Warning $line
        $line | Out-File -Append -Encoding utf8 "$OutDir\dialogs.txt"
        if ($script:no) { [void][DevBench.Win32]::PostMessage($script:no, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) }
        else { [void][DevBench.Win32]::PostMessage($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) }
    }
}
