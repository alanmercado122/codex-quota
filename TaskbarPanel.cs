using System;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;

namespace CodexQuota {
    // A separate, non-activating window. Never reparents into or resizes Explorer.
    class TaskbarPanel : Form {
        public Snapshot Data;
        public bool Remaining = true, Stale;
        public event Action OpenDetails;
        readonly System.Windows.Forms.Timer positionTimer = new System.Windows.Forms.Timer();
        readonly string settingsFile = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "CodexQuota", "panel-position.txt");
        int? offset;
        Point pressPoint, initialLocation;
        bool dragging, pressed;
        [StructLayout(LayoutKind.Sequential)] struct Rect { public int Left, Top, Right, Bottom; }
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindow(string cls, string name);
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string name);
        [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hwnd, out Rect rect);
        [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int count);
        [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
        protected override bool ShowWithoutActivation { get { return true; } }
        protected override CreateParams CreateParams {
            get { var cp = base.CreateParams; cp.ExStyle |= 0x08000000 | 0x00000080; return cp; } // WS_EX_NOACTIVATE | TOOLWINDOW
        }
        public TaskbarPanel(bool trackPosition) {
            Text = "Codex Quota — panel de barra";
            AccessibleName = "Porcentaje restante de cuota Codex de 5 horas y semanal";
            ClientSize = new Size(200, 42); AutoScaleMode = AutoScaleMode.None;
            FormBorderStyle = FormBorderStyle.None; ShowInTaskbar = false; TopMost = true;
            StartPosition = FormStartPosition.Manual;
            BackColor = Color.FromArgb(26, 30, 37); ForeColor = Color.White;
            DoubleBuffered = true; Cursor = Cursors.Hand;
            try { int value; if (File.Exists(settingsFile) && int.TryParse(File.ReadAllText(settingsFile), out value)) offset = value; } catch (IOException) { } catch (UnauthorizedAccessException) { }
            MouseDown += delegate(object sender, MouseEventArgs e) {
                if (e.Button != MouseButtons.Left) return;
                pressed = true; dragging = false; pressPoint = Cursor.Position; initialLocation = Location; Capture = true;
            };
            MouseMove += delegate {
                if (!pressed) return;
                var delta = new Size(Cursor.Position.X - pressPoint.X, Cursor.Position.Y - pressPoint.Y);
                if (Math.Abs(delta.Width) + Math.Abs(delta.Height) > 4) dragging = true;
                if (!dragging) return;
                Rectangle bar;
                if (!GetBar(out bar)) return;
                // Follow the taskbar's axis, and never allow dragging off its bounds.
                if (bar.Width >= bar.Height) Left = Math.Max(bar.Left, Math.Min(bar.Right - Width, initialLocation.X + delta.Width));
                else Top = Math.Max(bar.Top, Math.Min(bar.Bottom - Height, initialLocation.Y + delta.Height));
            };
            MouseUp += delegate(object sender, MouseEventArgs e) {
                if (e.Button != MouseButtons.Left || !pressed) return;
                pressed = false; Capture = false;
                if (dragging) {
                    Rectangle bar;
                    if (GetBar(out bar)) {
                        offset = bar.Width >= bar.Height ? Left - bar.Left : Top - bar.Top;
                        try { Directory.CreateDirectory(Path.GetDirectoryName(settingsFile)); File.WriteAllText(settingsFile, offset.Value.ToString()); } catch (IOException) { } catch (UnauthorizedAccessException) { }
                    }
                } else if (OpenDetails != null) OpenDetails();
                dragging = false;
            };
            if (trackPosition) {
                positionTimer.Interval = 2000;
                positionTimer.Tick += delegate { PositionOnBar(); };
                positionTimer.Start(); PositionOnBar();
            }
        }
        static bool GetBar(out Rectangle bar) {
            Rect r;
            var handle = FindWindow("Shell_TrayWnd", null);
            if (handle != IntPtr.Zero && GetWindowRect(handle, out r)) {
                bar = Rectangle.FromLTRB(r.Left, r.Top, r.Right, r.Bottom);
                return bar.Width > 0 && bar.Height > 0;
            }
            bar = Rectangle.Empty; return false;
        }
        public void ResetPosition() {
            offset = null;
            try { Directory.CreateDirectory(Path.GetDirectoryName(settingsFile)); File.WriteAllText(settingsFile, ""); } catch (IOException) { } catch (UnauthorizedAccessException) { }
            PositionOnBar();
        }
        void PositionOnBar() {
            if (pressed || IsDisposed) return;
            Rectangle bar;
            if (!GetBar(out bar)) { Hide(); return; }
            var screen = Screen.FromRectangle(bar).Bounds;
            // Follow autohide and yield to full-screen applications.
            var visible = Rectangle.Intersect(bar, screen);
            Rect fg;
            var foreground = GetForegroundWindow();
            var cls = new StringBuilder(128); GetClassName(foreground, cls, cls.Capacity);
            bool desktop = cls.ToString() == "Progman" || cls.ToString() == "WorkerW";
            bool full = !desktop && foreground != IntPtr.Zero && foreground != Handle && GetWindowRect(foreground, out fg)
                && fg.Left <= screen.Left && fg.Top <= screen.Top && fg.Right >= screen.Right && fg.Bottom >= screen.Bottom;
            if (visible.Width < 12 || visible.Height < 12 || full) { Hide(); return; }
            int x, y;
            if (bar.Width >= bar.Height) {
                Width = 200; Height = Math.Min(42, Math.Max(30, bar.Height - 4));
                Rect tray;
                var notification = FindWindowEx(FindWindow("Shell_TrayWnd", null), IntPtr.Zero, "TrayNotifyWnd", null);
                int end = notification != IntPtr.Zero && GetWindowRect(notification, out tray) ? tray.Left : bar.Right - 320;
                x = offset.HasValue ? bar.Left + offset.Value : end - Width - 10;
                x = Math.Max(bar.Left + 4, Math.Min(bar.Right - Width - 4, x));
                y = bar.Top + (bar.Height - Height) / 2;
            } else {
                Width = Math.Max(44, bar.Width - 4); Height = 84;
                x = bar.Left + 2;
                y = Math.Max(bar.Top + 4, Math.Min(bar.Bottom - Height - 4, offset.HasValue ? bar.Top + offset.Value : bar.Bottom - Height - 280));
            }
            Location = new Point(x, y);
            if (!Visible) Show();
            SetWindowPos(Handle, new IntPtr(-1), x, y, Width, Height, 0x0010 | 0x0040); // NOACTIVATE | SHOWWINDOW
        }
        static Color QuotaColor(WindowQuota quota, bool stale, Color normal) {
            return stale || quota == null || quota.Expired ? Color.FromArgb(146,154,166) : quota.Used >= 90 ? Color.FromArgb(255,130,137) : quota.Used >= 70 ? Color.FromArgb(255,204,105) : normal;
        }
        void Cell(Graphics g, Rectangle box, string label, WindowQuota quota, Color normal) {
            Color color = QuotaColor(quota, Stale, normal);
            string percent = quota == null ? "--" : quota.Percent(Remaining);
            using (var font = new Font("Segoe UI", 9, FontStyle.Regular, GraphicsUnit.Pixel))
                TextRenderer.DrawText(g, label, font, new Rectangle(box.X, box.Y + 3, 25, box.Height - 12), Color.FromArgb(171,181,196), TextFormatFlags.VerticalCenter | TextFormatFlags.NoPadding);
            using (var font = new Font("Segoe UI", 14, FontStyle.Bold, GraphicsUnit.Pixel))
                TextRenderer.DrawText(g, percent, font, new Rectangle(box.X + 27, box.Y, box.Width - 27, box.Height - 10), color, TextFormatFlags.Right | TextFormatFlags.VerticalCenter | TextFormatFlags.NoPadding);
            var track = new Rectangle(box.X, box.Bottom - 7, box.Width, 3);
            using (var brush = new SolidBrush(Color.FromArgb(62,69,82))) g.FillRectangle(brush, track);
            if (quota != null && !quota.Expired) using (var brush = new SolidBrush(color))
                g.FillRectangle(brush, track.X, track.Y, (float)(track.Width * (Remaining ? 100 - quota.Used : quota.Used) / 100), track.Height);
        }
        protected override void OnPaint(PaintEventArgs e) {
            base.OnPaint(e);
            var a = Data == null ? null : Data.Five; var b = Data == null ? null : Data.Week;
            using (var pen = new Pen(Color.FromArgb(61,69,83))) e.Graphics.DrawRectangle(pen, 0, 0, Width - 1, Height - 1);
            if (Width >= 150) {
                Cell(e.Graphics, new Rectangle(10, 2, (Width - 31) / 2, Height - 4), "5h", a, Color.FromArgb(111,227,191));
                Cell(e.Graphics, new Rectangle(Width / 2 + 6, 2, (Width - 31) / 2, Height - 4), "Sem", b, Color.FromArgb(147,185,255));
            } else {
                Cell(e.Graphics, new Rectangle(4, 2, Width - 8, Height / 2 - 3), "5h", a, Color.FromArgb(111,227,191));
                Cell(e.Graphics, new Rectangle(4, Height / 2, Width - 8, Height / 2 - 3), "S", b, Color.FromArgb(147,185,255));
            }
        }
        protected override void Dispose(bool disposing) {
            if (disposing) positionTimer.Dispose(); base.Dispose(disposing);
        }
    }
}
