using System.Runtime.InteropServices;

namespace GlideDock.Settings;

internal static class NativeMethods
{
    public const int WM_COPYDATA = 0x004A;
    private const ulong SettingsMagic = 0x474C4944; // "GLID"

    [StructLayout(LayoutKind.Sequential)]
    private struct COPYDATASTRUCT
    {
        public IntPtr dwData;
        public int cbData;
        public IntPtr lpData;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr SendMessageTimeout(
        IntPtr hWnd, int msg, IntPtr wParam, ref COPYDATASTRUCT lParam,
        uint flags, uint timeout, out IntPtr result);

    private const uint SMTO_ABORTIFHUNG = 0x0002;

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    public static extern uint GetDpiForWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetWindowPos(
        IntPtr hWnd, IntPtr hWndInsertAfter, int x, int y, int cx, int cy, uint flags);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

    public const uint SWP_NOSIZE = 0x0001;
    public const uint SWP_NOMOVE = 0x0002;
    public const uint SWP_NOZORDER = 0x0004;
    public const uint SWP_NOACTIVATE = 0x0010;

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern int MessageBox(IntPtr hWnd, string text, string caption, uint type);

    public static bool SendConfig(IntPtr owner, IntPtr sender, byte[] payload)
    {
        if (owner == IntPtr.Zero || sender == IntPtr.Zero || payload.Length == 0)
        {
            return false;
        }

        var handle = GCHandle.Alloc(payload, GCHandleType.Pinned);
        try
        {
            var data = new COPYDATASTRUCT
            {
                dwData = (IntPtr)SettingsMagic,
                cbData = payload.Length,
                lpData = handle.AddrOfPinnedObject()
            };
            // wParam identifies the sending window; the dock uses it to verify
            // that the payload comes from the settings process it spawned.
            var ok = SendMessageTimeout(
                owner, WM_COPYDATA, sender, ref data, SMTO_ABORTIFHUNG, 1000, out var result);
            return ok != IntPtr.Zero && result != IntPtr.Zero;
        }
        finally
        {
            handle.Free();
        }
    }
}
