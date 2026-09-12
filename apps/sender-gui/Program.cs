using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.Management;
using System.Net;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Threading.Tasks;
using System.Windows.Forms;
using System.Xml;

namespace LanAudio {
    static class Program {
        [STAThread] static void Main() {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new SenderForm());
        }
    }
    sealed class Source {
        public int Pid;
        public string ProcessName;
        public override string ToString() { return (ProcessName == "chrome" ? "Chrome／Chromeアプリ（YouTube Music等）" : "Spotify") + "  —  PID " + Pid; }
    }
    sealed class SenderForm : Form {
        readonly TextBox host = new TextBox();
        readonly NumericUpDown port = new NumericUpDown();
        readonly ComboBox source = new ComboBox();
        readonly Button refresh = new Button(), start = new Button(), stop = new Button();
        readonly Label status = new Label(), detail = new Label();
        readonly System.Windows.Forms.Timer timer = new System.Windows.Forms.Timer();
        readonly string folder = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "LanAudioSender");
        Process child;
        bool closing, stopping, refreshing;
        DateTime stoppingAt;
        string pendingProgress, lastError = "", currentLog;
        readonly object gate = new object();
        StreamWriter log;

        public SenderForm() {
            Text = "LAN Audio Sender";
            Font = new Font("Yu Gothic UI", 10F);
            AutoScaleMode = AutoScaleMode.Dpi;
            ClientSize = new Size(640, 430);
            MinimumSize = MaximumSize = Size;
            MaximizeBox = false;
            StartPosition = FormStartPosition.CenterScreen;
            BackColor = Color.FromArgb(246, 248, 251);
            AddLabel("LAN Audio Sender", 28, 22, 570, 38, 21, Color.FromArgb(23, 41, 67));
            AddLabel("Chrome・Chromeアプリ・Spotifyの音声をLANへ送信します。", 30, 67, 570, 26, 10, Color.DimGray);
            AddLabel("送信先 IPアドレス", 30, 113, 365, 24, 10, Color.Black);
            AddLabel("ポート", 425, 113, 180, 24, 10, Color.Black);
            host.SetBounds(30, 142, 365, 30); host.AccessibleName = "送信先 IPアドレス";
            port.SetBounds(425, 142, 180, 30); port.Minimum = 1; port.Maximum = 65535; port.Value = 40100;
            port.AccessibleName = "送信先ポート";
            Controls.Add(host); Controls.Add(port);
            AddLabel("取得するアプリ", 30, 189, 365, 24, 10, Color.Black);
            source.SetBounds(30, 217, 455, 32); source.DropDownStyle = ComboBoxStyle.DropDownList;
            source.AccessibleName = "取得するアプリ"; Controls.Add(source);
            source.SelectedIndexChanged += delegate { if (child == null) DescribeSource(); };
            refresh.Text = "再検索"; refresh.SetBounds(505, 216, 100, 34); refresh.Click += delegate { RefreshSources(); }; Controls.Add(refresh);
            start.Text = "送信開始"; start.SetBounds(30, 275, 275, 47); start.BackColor = Color.FromArgb(29, 91, 198);
            start.ForeColor = Color.White; start.FlatStyle = FlatStyle.Flat; start.FlatAppearance.BorderSize = 0;
            start.Click += delegate { StartSending(); }; Controls.Add(start);
            stop.Text = "停止"; stop.SetBounds(325, 275, 280, 47); stop.Enabled = false;
            stop.Click += delegate { StopSending(); }; Controls.Add(stop);
            status.SetBounds(30, 341, 575, 27); status.Text = "停止中"; status.Font = new Font(Font, FontStyle.Bold); Controls.Add(status);
            detail.SetBounds(30, 372, 575, 45); detail.Text = "受信先のアプリを起動してから送信を開始してください。"; detail.ForeColor = Color.DimGray; Controls.Add(detail);
            AcceptButton = start;
            LoadSettings();
            Shown += delegate { RefreshSources(); };
            FormClosing += OnClosing;
            timer.Interval = 200; timer.Tick += Tick; timer.Start();
        }
        void AddLabel(string text, int x, int y, int w, int h, float size, Color color) {
            Label label = new Label(); label.Text = text; label.SetBounds(x,y,w,h);
            label.Font = new Font(Font.FontFamily, size); label.ForeColor = color; Controls.Add(label);
        }
        void LoadSettings() {
            try {
                XmlDocument settings = new XmlDocument(); settings.XmlResolver = null;
                settings.Load(Path.Combine(folder, "settings.xml"));
                host.Text = settings.DocumentElement.GetAttribute("host");
                int saved;
                if (int.TryParse(settings.DocumentElement.GetAttribute("port"), out saved) && saved > 0 && saved <= 65535) port.Value = saved;
            } catch { /* A first launch or damaged optional settings uses defaults. */ }
        }
        void SaveSettings() {
            Directory.CreateDirectory(folder);
            using (XmlWriter writer = XmlWriter.Create(Path.Combine(folder,"settings.xml"), new XmlWriterSettings { Indent = true })) {
                writer.WriteStartElement("sender"); writer.WriteAttributeString("host",host.Text.Trim());
                writer.WriteAttributeString("port",port.Value.ToString()); writer.WriteEndElement();
            }
        }
        async void RefreshSources() {
            if (child != null || refreshing) return;
            refreshing = true; refresh.Enabled = start.Enabled = false;
            int selected = source.SelectedItem is Source ? ((Source)source.SelectedItem).Pid : 0;
            try {
                List<Source> found = await Task.Run(delegate {
                    Dictionary<int,int> parents = new Dictionary<int,int>();
                    Dictionary<int,string> names = new Dictionary<int,string>();
                    using (ManagementObjectSearcher search = new ManagementObjectSearcher("SELECT Name, ProcessId, ParentProcessId FROM Win32_Process WHERE Name='chrome.exe' OR Name='Spotify.exe'"))
                    using (ManagementObjectCollection results = search.Get()) {
                        foreach (ManagementObject item in results) using (item) {
                            parents[Convert.ToInt32(item["ProcessId"])] = Convert.ToInt32(item["ParentProcessId"]);
                            names[Convert.ToInt32(item["ProcessId"])] = Path.GetFileNameWithoutExtension(Convert.ToString(item["Name"])).ToLowerInvariant();
                        }
                    }
                    List<Source> list = new List<Source>();
                    foreach (KeyValuePair<int,int> pair in parents)
                        if (!names.ContainsKey(pair.Value) || names[pair.Value] != names[pair.Key])
                            list.Add(new Source { Pid = pair.Key, ProcessName = names[pair.Key] });
                    list.Sort(delegate(Source a, Source b) { int order = String.CompareOrdinal(a.ProcessName,b.ProcessName); return order != 0 ? order : a.Pid.CompareTo(b.Pid); }); return list;
                });
                if (IsDisposed) return;
                source.Items.Clear(); foreach (Source item in found) source.Items.Add(item);
                if (found.Count > 0) {
                    source.SelectedIndex = 0;
                    for (int i=0;i<found.Count;i++) if (found[i].Pid == selected) source.SelectedIndex=i;
                    DescribeSource();
                } else detail.Text = "ChromeまたはSpotifyを起動して「再検索」を押してください。";
            } catch (Exception e) { if (!IsDisposed) detail.Text = "アプリを検索できませんでした: " + e.Message; }
            finally { refreshing = false; if (!IsDisposed) { refresh.Enabled = true; start.Enabled = source.Items.Count > 0; } }
        }
        void DescribeSource() {
            Source selected = source.SelectedItem as Source;
            if (selected == null) return;
            detail.Text = selected.ProcessName == "chrome"
                ? "同じChromeのタブとChromeアプリをまとめて取得します。"
                : "Spotifyデスクトップアプリの音声を取得します。";
        }
        string ExtractEngine() {
            byte[] bytes;
            using (Stream resource = Assembly.GetExecutingAssembly().GetManifestResourceStream("sender-windows.exe")) {
                if (resource == null) throw new IOException("送信エンジンが見つかりません。");
                using (MemoryStream data = new MemoryStream()) { resource.CopyTo(data); bytes = data.ToArray(); }
            }
            string hash; using (SHA256 sha = SHA256.Create()) hash = BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
            string engineFolder = Path.Combine(folder, "engine", hash);
            Directory.CreateDirectory(engineFolder);
            string path = Path.Combine(engineFolder,"sender-windows.exe");
            bool valid = false;
            if (File.Exists(path)) using (SHA256 sha = SHA256.Create()) using (FileStream f = File.OpenRead(path)) valid = BitConverter.ToString(sha.ComputeHash(f)).Replace("-", "").ToLowerInvariant() == hash;
            if (!valid) File.WriteAllBytes(path, bytes);
            return path;
        }
        void StartSending() {
            if (child != null) return;
            IPAddress ip;
            if (!IPAddress.TryParse(host.Text.Trim(), out ip) || ip.Equals(IPAddress.Any) || ip.Equals(IPAddress.IPv6Any)) {
                MessageBox.Show(this,"送信先のIPアドレスを入力してください。","送信先を確認",MessageBoxButtons.OK,MessageBoxIcon.Information); host.Focus(); return;
            }
            Source selected = source.SelectedItem as Source;
            if (selected == null) { RefreshSources(); return; }
            Process process = null;
            try {
                using (Process target = Process.GetProcessById(selected.Pid)) if (!target.ProcessName.Equals(selected.ProcessName,StringComparison.OrdinalIgnoreCase)) throw new Exception("取得するアプリを再検索してください。");
                SaveSettings();
                string engine = ExtractEngine();
                string logs = Path.Combine(folder,"logs"); Directory.CreateDirectory(logs);
                currentLog = Path.Combine(logs,DateTime.Now.ToString("yyyyMMdd-HHmmss-fff") + "-" + Guid.NewGuid().ToString("N") + ".log");
                lock(gate) { log = new StreamWriter(currentLog,false,new UTF8Encoding(false)); lastError = ""; pendingProgress = null; }
                string destination = new IPEndPoint(ip,(int)port.Value).ToString();
                ProcessStartInfo info = new ProcessStartInfo(engine,"--backend process-include --pid " + selected.Pid + " --udp-to " + destination + " --seconds 86400 --control-stdin");
                info.UseShellExecute = false; info.CreateNoWindow = true;
                info.RedirectStandardInput = info.RedirectStandardOutput = info.RedirectStandardError = true;
                info.StandardOutputEncoding = info.StandardErrorEncoding = Encoding.UTF8;
                process = new Process(); process.StartInfo = info;
                process.OutputDataReceived += ReadOutput; process.ErrorDataReceived += ReadOutput;
                if (!process.Start()) throw new IOException("送信エンジンを起動できませんでした。");
                child = process; process.BeginOutputReadLine(); process.BeginErrorReadLine();
                host.Enabled = port.Enabled = source.Enabled = refresh.Enabled = start.Enabled = false;
                stop.Enabled = true; stopping = false;
                status.Text = "開始しています…"; detail.Text = destination + " へ送信します。";
            } catch (Exception e) {
                if (process != null) { try { if (!process.HasExited) process.Kill(); } catch {} process.Dispose(); }
                child = null; CloseLog();
                MessageBox.Show(this,e.Message,"開始できませんでした",MessageBoxButtons.OK,MessageBoxIcon.Error);
                SetIdle();
            }
        }
        void ReadOutput(object sender, DataReceivedEventArgs e) {
            if (e.Data == null) return;
            lock(gate) {
                try { if (log != null) log.WriteLine(e.Data); } catch { /* Capture must not wait for a failing log device. */ }
                if (e.Data.StartsWith("packets=")) pendingProgress = e.Data;
                if (e.Data.StartsWith("Error:") || e.Data.StartsWith("    ")) lastError = e.Data.Trim();
            }
        }
        void StopSending() {
            if (child == null || stopping) return;
            stopping = true; stoppingAt = DateTime.UtcNow; stop.Enabled = false; status.Text = "停止しています…";
            try { child.StandardInput.WriteLine("stop"); child.StandardInput.Flush(); } catch { /* Exit is checked by the timer. */ }
        }
        void Tick(object sender, EventArgs args) {
            if (child == null) return;
            if (child.HasExited) {
                child.WaitForExit(); int code = child.ExitCode; child.Dispose(); child = null; CloseLog();
                bool requested = stopping; stopping = false; SetIdle();
                status.Text = code == 0 ? "停止中" : "送信が終了しました";
                if (code != 0) { string error; lock(gate) error=lastError; detail.Text = error.Length > 0 ? error : "送信エラー。ログ: " + currentLog; }
                else detail.Text = requested ? "送信を停止しました。設定を変更して再開できます。" : "送信を終了しました（連続送信は最大24時間）。";
                if (closing) Close(); return;
            }
            if (stopping) {
                if ((DateTime.UtcNow-stoppingAt).TotalSeconds > 10) {
                    try { child.Kill(); } catch {} detail.Text = "終了応答がないため送信プロセスを終了しています。";
                }
                return;
            }
            string progress; lock(gate) { progress = pendingProgress; pendingProgress = null; }
            if (progress != null) { status.Text = "送信中"; detail.Text = host.Text.Trim() + ":" + port.Value + " へ送信中。受信先で音声を確認してください。"; }
        }
        void SetIdle() { host.Enabled = port.Enabled = source.Enabled = refresh.Enabled = true; start.Enabled = source.Items.Count > 0; stop.Enabled = false; }
        void CloseLog() { lock(gate) { if (log != null) { try { log.Dispose(); } catch {} log = null; } } }
        void OnClosing(object sender, FormClosingEventArgs e) {
            if (child != null) { closing = true; e.Cancel = true; StopSending(); }
            else { timer.Stop(); CloseLog(); }
        }
    }
}
