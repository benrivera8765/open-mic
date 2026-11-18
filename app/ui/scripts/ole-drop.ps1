# SPDX-License-Identifier: GPL-3.0-or-later
# A real OS-level (OLE) drag-and-drop of files onto a running app window, for the
# real-app feature drive (real-app-features.mjs, WORK_ORDER 4.1 "drag-and-drop").
#
#   powershell -NoProfile -STA -ExecutionPolicy Bypass -File ole-drop.ps1 -TargetPid <pid> -Files <path>[,<path>...]
#
# A small helper window starts a standard Windows drag (DoDragDrop with a CF_HDROP file
# list, as Explorer does) under a synthesized left-button press; a second thread moves
# the real mouse pointer onto the target window and releases the button there. Before it
# releases, it checks that the window under the pointer belongs to the target app
# (GetAncestor(WindowFromPoint) == the app's main window); otherwise it presses Escape,
# which cancels the drag, so nothing is ever dropped anywhere else. The pointer is put
# back where it was. Needs an unlocked interactive desktop.
# Prints one JSON line: { ok, effect, target, point, cursor_ok, error }.
param(
    [Parameter(Mandatory = $true)][int]$TargetPid,
    [Parameter(Mandatory = $true)][string[]]$Files
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;
using System.Collections.Specialized;

public static class JkOleDrop {
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr e);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint f, UIntPtr e);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    const uint LEFTDOWN = 0x2, LEFTUP = 0x4, MOVE = 0x1;

    public static string Run(IntPtr target, string[] files) {
        SetProcessDPIAware();
        string error = null; string effect = "none"; bool released = false; bool cursorOk = true;
        RECT r; GetWindowRect(target, out r);
        int tx = (r.L + r.R) / 2, ty = r.T + (r.B - r.T) * 2 / 3;
        POINT start; GetCursorPos(out start);
        // Bring the app's window to the top of the z-order (topmost, then back to normal),
        // which works even when Windows refuses to hand over the foreground.
        ShowWindow(target, 9 /* SW_RESTORE */);
        SetWindowPos(target, new IntPtr(-1) /* HWND_TOPMOST */, 0, 0, 0, 0, 0x0003 /* NOSIZE|NOMOVE */);
        SetWindowPos(target, new IntPtr(-2) /* HWND_NOTOPMOST */, 0, 0, 0, 0, 0x0003);
        SetForegroundWindow(target);
        Thread.Sleep(400);
        var form = new Form();
        form.FormBorderStyle = FormBorderStyle.None; form.TopMost = true; form.ShowInTaskbar = false;
        form.StartPosition = FormStartPosition.Manual; form.BackColor = Color.DarkGray;
        form.Bounds = new Rectangle(Math.Max(0, r.L - 120), Math.Max(0, r.T + 40), 100, 100);
        if (form.Bounds.IntersectsWith(new Rectangle(tx - 5, ty - 5, 10, 10))) form.Location = new Point(r.L + 10, r.T + 40);
        form.MouseDown += (s, e) => {
            var list = new StringCollection(); list.AddRange(files);
            var data = new DataObject(); data.SetFileDropList(list);
            var mover = new Thread(() => {
                try {
                    Thread.Sleep(300);
                    POINT p0; GetCursorPos(out p0);
                    for (int i = 1; i <= 30; i++) {
                        int x = p0.X + (tx - p0.X) * i / 30, y = p0.Y + (ty - p0.Y) * i / 30;
                        if (!SetCursorPos(x, y)) cursorOk = false;
                        mouse_event(MOVE, 0, 0, 0, UIntPtr.Zero);
                        Thread.Sleep(25);
                    }
                    for (int i = 0; i < 8; i++) { mouse_event(MOVE, (i % 2 == 0) ? 1 : -1, 0, 0, UIntPtr.Zero); Thread.Sleep(60); }
                    POINT here; GetCursorPos(out here);
                    IntPtr root = GetAncestor(WindowFromPoint(here), 2 /* GA_ROOT */);
                    if (root != target) {
                        error = "the window under the pointer is not the app's window; drag cancelled";
                        keybd_event(0x1B, 0, 0, UIntPtr.Zero); keybd_event(0x1B, 0, 2, UIntPtr.Zero);
                        Thread.Sleep(100);
                    }
                    mouse_event(LEFTUP, 0, 0, 0, UIntPtr.Zero); released = true;
                } catch (Exception ex) { error = ex.ToString(); mouse_event(LEFTUP, 0, 0, 0, UIntPtr.Zero); released = true; }
            });
            mover.IsBackground = true; mover.Start();
            effect = form.DoDragDrop(data, DragDropEffects.Copy | DragDropEffects.Link | DragDropEffects.Move).ToString();
            form.BeginInvoke(new Action(() => form.Close()));
        };
        form.Shown += (s, e) => {
            var c = form.PointToScreen(new Point(50, 50));
            if (!SetCursorPos(c.X, c.Y)) cursorOk = false;
            Thread.Sleep(150);
            mouse_event(LEFTDOWN, 0, 0, 0, UIntPtr.Zero);
        };
        var guard = new System.Windows.Forms.Timer(); guard.Interval = 20000;
        guard.Tick += (s, e) => { if (error == null) error = "timed out"; if (!released) mouse_event(LEFTUP, 0, 0, 0, UIntPtr.Zero); form.Close(); };
        guard.Start();
        Application.Run(form);
        SetCursorPos(start.X, start.Y);
        return "{\"ok\":" + (error == null && effect != "None" ? "true" : "false") + ",\"effect\":\"" + effect + "\",\"target\":\"0x" +
            target.ToString("X") + "\",\"point\":[" + tx + "," + ty + "],\"cursor_ok\":" + (cursorOk ? "true" : "false") +
            ",\"error\":" + (error == null ? "null" : "\"" + error.Replace("\\", "\\\\").Replace("\"", "'").Replace("\r", " ").Replace("\n", " ") + "\"") + "}";
    }
}
"@
$p = Get-Process -Id $TargetPid
$h = $p.MainWindowHandle
if ($h -eq [IntPtr]::Zero) { Write-Output '{"ok":false,"error":"the app has no main window"}'; exit 1 }
$full = @($Files | ForEach-Object { (Resolve-Path -LiteralPath $_).ProviderPath })
Write-Output ([JkOleDrop]::Run($h, $full))
