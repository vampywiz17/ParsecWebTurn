# Send the title-bar X close request to our smoke-test process only.
# This needs no additional production IPC permission.
param([Parameter(Mandatory)][ValidateRange(1, 2147483647)][int]$TestProcessId)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class SmokeWindowClose {
    private delegate bool EnumCallback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumWindows(EnumCallback callback, IntPtr data);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] private static extern int GetWindowText(IntPtr window, StringBuilder title, int count);
    [DllImport("user32.dll", SetLastError=true)] private static extern bool PostMessage(IntPtr window, uint message, IntPtr wparam, IntPtr lparam);
    public static void CloseParsec(uint process) {
        IntPtr selected = IntPtr.Zero;
        int matches = 0;
        EnumWindows((window, data) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            if (owner != process) return true;
            var title = new StringBuilder(512); GetWindowText(window, title, title.Capacity);
            if (!title.ToString().StartsWith("Parsec ", StringComparison.Ordinal)) return true;
            matches++; selected = window; return true;
        }, IntPtr.Zero);
        if (matches != 1) throw new Exception("Expected one Parsec window owned by smoke-test process");
        if (!PostMessage(selected, 0x0010, IntPtr.Zero, IntPtr.Zero))
            throw new Exception("Cannot post native close request");
    }
}
'@
[SmokeWindowClose]::CloseParsec([uint32]$TestProcessId)
