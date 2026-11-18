using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;
using System.Web.Script.Serialization;
using System.Windows.Forms;
using Microsoft.Win32;

[assembly: AssemblyTitle("Codex Quota")]
[assembly: AssemblyDescription("Cuota de Codex · 5 horas y semanal")]
[assembly: AssemblyProduct("Codex Quota")]
[assembly: AssemblyVersion("1.0.0.0")]

namespace CodexQuota {
    class WindowQuota {
        public double Used;
        public long? Reset;
        public bool Expired { get { return Reset.HasValue && Reset.Value <= DateTimeOffset.UtcNow.ToUnixTimeSeconds(); } }
        public string Percent(bool remaining) { return Expired ? "--" : Math.Round(remaining ? 100 - Used : Used).ToString("0") + "%"; }
        public string Countdown() {
            if (!Reset.HasValue) return "Reinicio no disponible";
            if (Expired) return "Reinicio pendiente de confirmar";
            var t = TimeSpan.FromSeconds(Reset.Value - DateTimeOffset.UtcNow.ToUnixTimeSeconds());
            return "Reinicia en " + (t.TotalDays >= 1 ? (int)t.TotalDays + "d " + t.Hours + "h" : (int)t.TotalHours + "h " + t.Minutes + "m");
        }
    }
    class Snapshot {
        public WindowQuota Five, Week;
        public DateTime Updated = DateTime.Now;
    }
    static class Reader {
        public static Dictionary<string, object> Obj(object o) { return o as Dictionary<string, object>; }
        public static object Get(Dictionary<string, object> d, string k) { object v; return d != null && d.TryGetValue(k, out v) ? v : null; }
        public static Snapshot Parse(Dictionary<string, object> result) {
            var buckets = Obj(Get(result, "rateLimitsByLimitId"));
            var bucket = buckets != null ? Obj(Get(buckets, "codex")) : Obj(Get(result, "rateLimits"));
            if (bucket == null) throw new InvalidDataException("La cuenta no ofrece cuota general de Codex.");
            var limitId = Get(bucket, "limitId") as string;
            if (limitId != null && limitId != "codex") throw new InvalidDataException("La respuesta no contiene la cuota general de Codex.");
            var s = new Snapshot();
            foreach (string key in new[] { "primary", "secondary" }) {
                var w = Obj(Get(bucket, key));
                if (w == null || Get(w, "usedPercent") == null || Get(w, "windowDurationMins") == null) continue;
                var used = Convert.ToDouble(w["usedPercent"]);
                if (double.IsNaN(used) || double.IsInfinity(used) || used < 0 || used > 100) continue;
                var q = new WindowQuota { Used = used, Reset = Get(w, "resetsAt") == null ? (long?)null : Convert.ToInt64(w["resetsAt"]) };
                int mins = Convert.ToInt32(w["windowDurationMins"]);
                if (mins == 300) s.Five = q;
                if (mins == 10080) s.Week = q;
            }
            if (s.Five == null && s.Week == null) throw new InvalidDataException("No hay ventanas de 5 horas o semanal disponibles.");
            return s;
        }
        static string FindCodex() {
            string local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
            foreach (string p in new[] { Path.Combine(local, "Programs", "OpenAI", "Codex", "bin", "codex.exe"), Path.Combine(local, "OpenAI", "Codex", "bin", "codex.exe") })
                if (File.Exists(p)) return p;
            foreach (var dir in (Environment.GetEnvironmentVariable("PATH") ?? "").Split(';')) {
                try { string p = Path.Combine(dir.Trim('"'), "codex.exe"); if (File.Exists(p)) return p; } catch (ArgumentException) { }
            }
            throw new FileNotFoundException("No se encontró Codex. Instálalo e inicia sesión.");
        }
        static async Task<Dictionary<string, object>> Response(Process p, int id, JavaScriptSerializer json) {
            while (true) {
                string line = await p.StandardOutput.ReadLineAsync().ConfigureAwait(false);
                if (line == null) throw new IOException("Codex cerró la conexión.");
                Dictionary<string, object> msg;
                try { msg = json.Deserialize<Dictionary<string, object>>(line); } catch (ArgumentException) { continue; }
                if (Get(msg, "id") == null || Convert.ToString(msg["id"]) != id.ToString()) continue;
                // Do not expose server errors: they may contain account information.
                if (Get(msg, "error") != null) throw new InvalidOperationException("Codex no pudo consultar la cuota. Revisa tu sesión o conexión.");
                var result = Obj(Get(msg, "result"));
                if (result == null) throw new InvalidDataException("Respuesta de Codex no válida.");
                return result;
            }
        }
        public static async Task<Snapshot> Read() {
            using (var p = new Process()) {
                p.StartInfo = new ProcessStartInfo(FindCodex(), "--no-daemon app-server --stdio") {
                    UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true,
                    RedirectStandardOutput = true, RedirectStandardError = true,
                    WorkingDirectory = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile)
                };
                p.ErrorDataReceived += delegate { }; // Drain, never persist authentication or diagnostics.
                p.Start(); p.BeginErrorReadLine();
                using (var deadline = new CancellationTokenSource(TimeSpan.FromSeconds(30)))
                using (deadline.Token.Register(delegate { try { if (!p.HasExited) p.Kill(); } catch (InvalidOperationException) { } })) {
                    try {
                        var json = new JavaScriptSerializer();
                        p.StandardInput.AutoFlush = true;
                        await p.StandardInput.WriteLineAsync("{\"id\":1,\"method\":\"initialize\",\"params\":{\"clientInfo\":{\"name\":\"codex_quota_tray\",\"title\":\"Codex Quota\",\"version\":\"1.0.0\"}}}").ConfigureAwait(false);
                        await Response(p, 1, json).ConfigureAwait(false);
                        await p.StandardInput.WriteLineAsync("{\"method\":\"initialized\"}").ConfigureAwait(false);
                        await p.StandardInput.WriteLineAsync("{\"id\":2,\"method\":\"account/rateLimits/read\"}").ConfigureAwait(false);
                        return Parse(await Response(p, 2, json).ConfigureAwait(false));
                    } finally {
                        try { p.StandardInput.Close(); if (!p.WaitForExit(1500)) { p.Kill(); p.WaitForExit(1500); } } catch (InvalidOperationException) { }
                    }
                }
            }
        }
    }
    class Popup : Form {
        public Snapshot Data;
        public bool Remaining = true, Stale;
        public string Status = "Consultando cuota…";
        readonly Color muted = Color.FromArgb(156, 166, 183);
        public Popup() {
            Text = "Codex Quota"; ClientSize = new Size(288, 224);
            FormBorderStyle = FormBorderStyle.None; ShowInTaskbar = false; TopMost = true;
            BackColor = Color.FromArgb(23, 27, 34); ForeColor = Color.White;
            Font = new Font("Segoe UI", 9); DoubleBuffered = true;
            AccessibleName = "Cuota de Codex";
            var refresh = new Button { Text = "Actualizar", FlatStyle = FlatStyle.Flat, Bounds = new Rectangle(18, 179, 116, 28), ForeColor = ForeColor, BackColor = BackColor };
            refresh.FlatAppearance.BorderColor = Color.FromArgb(64, 73, 88);
            refresh.Click += delegate { if (RefreshRequested != null) RefreshRequested(); };
            Controls.Add(refresh);
            var close = new Button { Text = "Cerrar", FlatStyle = FlatStyle.Flat, Bounds = new Rectangle(150, 179, 116, 28), ForeColor = muted };
            close.FlatAppearance.BorderSize = 0; close.Click += delegate { Hide(); }; Controls.Add(close);
            Deactivate += delegate { Hide(); };
            KeyPreview = true; KeyDown += delegate(object sender, KeyEventArgs e) { if (e.KeyCode == Keys.Escape) Hide(); };
        }
        public event Action RefreshRequested;
        public void OpenNearTray() {
            var area = Screen.FromPoint(Cursor.Position).WorkingArea;
            Location = new Point(Math.Max(area.Left, Math.Min(Cursor.Position.X - Width / 2, area.Right - Width - 8)), area.Bottom - Height - 8);
            Show(); Activate(); Invalidate();
        }
        void LabelText(Graphics g, string text, float size, Color color, Rectangle rect, FontStyle style) {
            using (var f = new Font("Segoe UI", size, style)) TextRenderer.DrawText(g, text, f, rect, color, TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.EndEllipsis | TextFormatFlags.NoPrefix);
        }
        void Row(Graphics g, string title, WindowQuota q, int y, Color color) {
            LabelText(g, title, 9, muted, new Rectangle(18,y,160,22), FontStyle.Regular);
            string val = q == null ? "--" : q.Percent(Remaining);
            LabelText(g, val, 13, Color.White, new Rectangle(213,y-2,62,26), FontStyle.Bold);
            using (var b = new SolidBrush(Color.FromArgb(48,55,67))) g.FillRectangle(b,18,y+29,248,4);
            if (q != null && !q.Expired) using (var b = new SolidBrush(Stale ? muted : color)) g.FillRectangle(b,18,y+29,(float)(248 * (Remaining ? 100-q.Used : q.Used) / 100),4);
            LabelText(g, q == null ? "Sin datos para esta ventana" : q.Countdown(), 8, muted, new Rectangle(18,y+36,248,18), FontStyle.Regular);
        }
        protected override void OnPaint(PaintEventArgs e) {
            base.OnPaint(e); var g = e.Graphics;
            using (var pen = new Pen(Color.FromArgb(57,65,80))) g.DrawRectangle(pen,0,0,Width-1,Height-1);
            LabelText(g,"CODEX",10,Color.White,new Rectangle(18,10,80,24),FontStyle.Bold);
            LabelText(g,Remaining ? "CUOTA RESTANTE" : "CUOTA USADA",8,muted,new Rectangle(120,10,150,24),FontStyle.Regular);
            Row(g,"5 horas",Data == null ? null : Data.Five,42,Color.FromArgb(104,220,181));
            Row(g,"Semanal",Data == null ? null : Data.Week,105,Color.FromArgb(139,180,255));
            LabelText(g,Status,7,muted,new Rectangle(18,161,248,16),FontStyle.Regular);
        }
    }
    class TrayApp : ApplicationContext {
        readonly NotifyIcon tray = new NotifyIcon();
        readonly Popup popup = new Popup();
        readonly TaskbarPanel taskbar = new TaskbarPanel(true);
        readonly System.Windows.Forms.Timer poll = new System.Windows.Forms.Timer();
        readonly System.Windows.Forms.Timer clock = new System.Windows.Forms.Timer();
        bool busy, exiting, remaining = true, stale;
        Snapshot data;
        Icon current;
        const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
        [DllImport("user32.dll")] static extern bool DestroyIcon(IntPtr handle);
        static Color Tint(WindowQuota q, bool stale) { return stale || q == null || q.Expired ? Color.Gray : q.Used >= 90 ? Color.FromArgb(255,133,133) : q.Used >= 70 ? Color.FromArgb(255,208,112) : Color.FromArgb(130,238,201); }
        public TrayApp() {
            // Create a UI synchronization context before the asynchronous initial query.
            var unused = popup.Handle;
            var menu = new ContextMenuStrip();
            menu.Items.Add("Restablecer posición del panel", null, delegate { taskbar.ResetPosition(); });
            menu.Items.Add("Ver cuota", null, delegate { popup.OpenNearTray(); });
            menu.Items.Add("Actualizar ahora", null, async delegate { await Update(); });
            var mode = new ToolStripMenuItem("Mostrar porcentaje restante") { Checked = true, CheckOnClick = true };
            mode.Click += delegate { remaining = mode.Checked; Render(); }; menu.Items.Add(mode);
            var startup = new ToolStripMenuItem("Iniciar con Windows") { CheckOnClick = true };
            using (var k = Registry.CurrentUser.OpenSubKey(RunKey)) startup.Checked = k != null && k.GetValue("CodexQuota") != null;
            startup.Click += delegate {
                try { using (var k = Registry.CurrentUser.CreateSubKey(RunKey)) { if (startup.Checked) k.SetValue("CodexQuota", "\"" + Application.ExecutablePath + "\""); else k.DeleteValue("CodexQuota", false); } }
                catch { startup.Checked = !startup.Checked; MessageBox.Show("No se pudo cambiar el inicio automático.", "Codex Quota"); }
            }; menu.Items.Add(startup);
            menu.Items.Add("Configuración de la barra de tareas", null, delegate { Process.Start(new ProcessStartInfo("ms-settings:taskbar") { UseShellExecute = true }); });
            menu.Items.Add(new ToolStripSeparator()); menu.Items.Add("Salir", null, delegate { ExitThread(); });
            tray.ContextMenuStrip = menu;
            taskbar.ContextMenuStrip = menu;
            taskbar.OpenDetails += delegate { popup.OpenNearTray(); };
            tray.MouseClick += delegate(object sender, MouseEventArgs e) { if (e.Button == MouseButtons.Left) { if (popup.Visible) popup.Hide(); else popup.OpenNearTray(); } };
            popup.RefreshRequested += async delegate { await Update(); };
            Render(); tray.Visible = true;
            poll.Interval = 300000; poll.Tick += async delegate { await Update(); }; poll.Start();
            clock.Interval = 60000; clock.Tick += delegate { Render(); }; clock.Start();
            SynchronizationContext.Current.Post(async delegate { await Update(); }, null);
        }
        async Task Update() {
            if (busy || exiting) return;
            busy = true; popup.Status = "Consultando cuota…"; popup.Invalidate();
            try { data = await Reader.Read(); stale = false; popup.Status = "Actualizado " + data.Updated.ToString("HH:mm") + " · cada 5 min"; }
            catch { stale = true; popup.Status = data == null ? "Sin conexión o sesión. Reintentar: Actualizar." : "Sin conexión · último dato " + data.Updated.ToString("HH:mm"); }
            finally { busy = false; if (!exiting) Render(); }
        }
        void Render() {
            taskbar.Data = data; taskbar.Remaining = remaining; taskbar.Stale = stale; taskbar.Invalidate();
            popup.Data = data; popup.Remaining = remaining; popup.Stale = stale; popup.Invalidate();
            var a = data == null ? null : data.Five; var b = data == null ? null : data.Week;
            string pa = a == null ? "--" : a.Percent(remaining), pb = b == null ? "--" : b.Percent(remaining);
            popup.AccessibleDescription = "Cuota " + (remaining ? "restante" : "usada") + ": 5 horas " + pa + ", semanal " + pb + (stale ? ". Datos sin actualizar." : ".");
            taskbar.AccessibleDescription = popup.AccessibleDescription;
            string tip = "Codex " + (remaining ? "restante" : "usado") + ": 5h " + pa + " · semana " + pb + (stale ? " · SIN ACTUALIZAR" : "");
            tray.Text = tip.Length > 63 ? tip.Substring(0,63) : tip;
            using (var bmp = new Bitmap(32,32)) using (var g = Graphics.FromImage(bmp)) {
                g.Clear(Color.Transparent); g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.SingleBitPerPixelGridFit;
                using (var font = new Font("Segoe UI",10,FontStyle.Bold,GraphicsUnit.Pixel)) {
                    TextRenderer.DrawText(g,pa.Replace("%",""),font,new Rectangle(0,0,32,15),Tint(a,stale),TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.NoPadding);
                    TextRenderer.DrawText(g,pb.Replace("%",""),font,new Rectangle(0,17,32,15),Tint(b,stale),TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.NoPadding);
                }
                using (var pen = new Pen(Color.FromArgb(110,130,145))) g.DrawLine(pen,7,16,25,16);
                IntPtr h = bmp.GetHicon(); Icon next;
                try { using (var tmp = Icon.FromHandle(h)) next = (Icon)tmp.Clone(); } finally { DestroyIcon(h); }
                tray.Icon = next; if (current != null) current.Dispose(); current = next;
            }
        }
        protected override void ExitThreadCore() {
            exiting = true; poll.Stop(); clock.Stop(); poll.Dispose(); clock.Dispose();
            taskbar.Dispose(); tray.Visible = false; tray.Dispose(); if (current != null) current.Dispose(); popup.Dispose(); base.ExitThreadCore();
        }
    }
    static class Program {
        [STAThread] static int Main(string[] args) {
            if (args.Length > 0 && args[0] == "--preview") {
                Application.EnableVisualStyles(); Application.SetCompatibleTextRenderingDefault(false);
                using (var panel = new Popup()) {
                    panel.Data = Reader.Read().GetAwaiter().GetResult();
                    panel.Status = "Actualizado " + panel.Data.Updated.ToString("HH:mm") + " · cada 5 min";
                    using (var bmp = new Bitmap(panel.Width, panel.Height)) {
                        panel.DrawToBitmap(bmp, new Rectangle(Point.Empty, panel.Size));
                        foreach (Control control in panel.Controls) control.DrawToBitmap(bmp, control.Bounds);
                        bmp.Save(Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "preview.png"));
                    }
                    using (var strip = new TaskbarPanel(false)) using (var bmp = new Bitmap(strip.Width, strip.Height)) {
                        strip.Data = panel.Data;
                        strip.DrawToBitmap(bmp, new Rectangle(Point.Empty, strip.Size));
                        bmp.Save(Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "taskbar-preview.png"));
                    }
                } return 0;
            }
            if (args.Length > 0 && args[0] == "--probe") {
                try {
                    var s = Reader.Read().GetAwaiter().GetResult();
                    File.WriteAllText(Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"probe.json"), new JavaScriptSerializer().Serialize(new {
                        fiveHourUsedPercent = s.Five == null ? (double?)null : s.Five.Used,
                        weeklyUsedPercent = s.Week == null ? (double?)null : s.Week.Used,
                        fiveHourReset = s.Five == null ? null : s.Five.Reset,
                        weeklyReset = s.Week == null ? null : s.Week.Reset,
                        updatedAt = s.Updated.ToString("o")
                    })); return 0;
                } catch (Exception e) { File.WriteAllText(Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"probe.json"), new JavaScriptSerializer().Serialize(new { error = e.Message })); return 1; }
            }
            if (args.Length > 0 && args[0] == "--self-test") return SelfTest();
            bool created;
            using (var mutex = new Mutex(true,"Local\\CodexQuotaTray",out created)) {
                if (!created) return 0;
                Application.EnableVisualStyles(); Application.SetCompatibleTextRenderingDefault(false);
                Application.Run(new TrayApp()); GC.KeepAlive(mutex);
            } return 0;
        }
        static int SelfTest() {
            var json = new JavaScriptSerializer();
            Func<string,Snapshot> parse = s => Reader.Parse(json.Deserialize<Dictionary<string,object>>(s));
            try {
                var a = parse("{\"rateLimits\":{\"primary\":{\"usedPercent\":25,\"windowDurationMins\":300},\"secondary\":{\"usedPercent\":80,\"windowDurationMins\":10080}}}");
                if (a.Five.Percent(true) != "75%" || a.Week.Percent(false) != "80%") return 1;
                var b = parse("{\"rateLimitsByLimitId\":{\"codex\":{\"secondary\":{\"usedPercent\":0,\"windowDurationMins\":300}}}}");
                if (b.Five.Used != 0 || b.Week != null) return 2;
                foreach (string invalid in new[] { "{}", "{\"rateLimits\":{\"primary\":{\"usedPercent\":12,\"windowDurationMins\":15}}}", "{\"rateLimitsByLimitId\":{\"other\":{}},\"rateLimits\":{}}", "{\"rateLimits\":{\"primary\":{\"usedPercent\":101,\"windowDurationMins\":300}}}" }) {
                    bool rejected = false; try { parse(invalid); } catch (InvalidDataException) { rejected = true; }
                    if (!rejected) return 3;
                }
                if (new WindowQuota { Used = 80, Reset = 1 }.Percent(true) != "--") return 4;
                return 0;
            } catch { return 5; }
        }
    }
}
