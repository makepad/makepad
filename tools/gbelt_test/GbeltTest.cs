using System;
using System.Drawing;
using System.Globalization;
using System.IO.Ports;
using System.Text;
using System.Threading;
using System.Windows.Forms;

namespace GbeltTest
{
    static class Program
    {
        [STAThread]
        static void Main()
        {
            Application.EnableVisualStyles();
            Application.SetCompatibleTextRenderingDefault(false);
            Application.Run(new MainForm());
        }
    }

    sealed class MainForm : Form
    {
        readonly ComboBox _port = new ComboBox();
        readonly ComboBox _baud = new ComboBox();
        readonly Button _connect = new Button();
        readonly CheckBox _stream = new CheckBox();
        readonly CheckBox _invertLeft = new CheckBox();
        readonly TrackBar _right = new TrackBar();
        readonly TrackBar _left = new TrackBar();
        readonly Label _rightVal = new Label();
        readonly Label _leftVal = new Label();
        readonly Label _pkt = new Label();
        readonly Label _tx = new Label();
        readonly TextBox _log = new TextBox();
        readonly System.Windows.Forms.Timer _timer = new System.Windows.Forms.Timer();
        readonly System.Windows.Forms.Timer _tug = new System.Windows.Forms.Timer();
        readonly object _io = new object();

        SerialPort _sp;
        int _txCount;
        byte _seq = 1;
        bool _ready;
        bool _initing;
        int _tugWhich; // 1=right 2=left
        int _tugSaved;

        const int Rest = 16384;
        const byte Stx = 0x02;
        const byte Etx = 0x03;
        const byte Footer = 0x7D;

        public MainForm()
        {
            Text = "G-Belt test v3 (921600, captured init + 64-byte S stream)";
            Width = 640;
            Height = 720;
            FormBorderStyle = FormBorderStyle.FixedSingle;
            MaximizeBox = false;

            int y = 12;
            Controls.Add(Lbl("Port", 12, y + 4));
            _port.DropDownStyle = ComboBoxStyle.DropDownList;
            _port.SetBounds(52, y, 90, 24);
            foreach (var p in SerialPort.GetPortNames()) _port.Items.Add(p);
            if (_port.Items.Contains("COM3")) _port.SelectedItem = "COM3";
            else if (_port.Items.Count > 0) _port.SelectedIndex = 0;
            Controls.Add(_port);

            Controls.Add(Lbl("Baud", 150, y + 4));
            _baud.DropDownStyle = ComboBoxStyle.DropDownList;
            _baud.SetBounds(190, y, 90, 24);
            foreach (var b in new[] { 115200, 230400, 460800, 921600 })
                _baud.Items.Add(b);
            _baud.SelectedItem = 921600;
            Controls.Add(_baud);

            _connect.Text = "Connect + init";
            _connect.SetBounds(300, y, 120, 28);
            _connect.Click += ConnectClick;
            Controls.Add(_connect);

            var refresh = new Button { Text = "Ports", Left = 430, Top = y, Width = 70, Height = 28 };
            refresh.Click += (s, e) => RefreshPorts();
            Controls.Add(refresh);

            y += 36;
            _stream.Text = "Stream 50 Hz (S+O+F+F)";
            _stream.SetBounds(12, y, 220, 22);
            _stream.Checked = true;
            Controls.Add(_stream);

            _invertLeft.Text = "Invert left (Commander does)";
            _invertLeft.SetBounds(240, y, 220, 22);
            _invertLeft.Checked = true;
            Controls.Add(_invertLeft);

            _tx.SetBounds(460, y, 150, 22);
            _tx.Text = "TX 0";
            Controls.Add(_tx);

            y += 28;
            Controls.Add(Lbl("G-Belt Right  (port 1 / S442)", 12, y));
            _right.Minimum = 0;
            _right.Maximum = 32767;
            _right.TickFrequency = 4096;
            _right.SetBounds(12, y + 16, 460, 32);
            _right.AutoSize = false;
            _right.Value = Rest;
            _right.Scroll += (s, e) => { ShowVal(_right, _rightVal); Kick(); };
            Controls.Add(_right);
            _rightVal.SetBounds(480, y + 20, 130, 22);
            Controls.Add(_rightVal);
            ShowVal(_right, _rightVal);

            y += 56;
            Controls.Add(Lbl("G-Belt Left   (port 2 / S441)", 12, y));
            _left.Minimum = 0;
            _left.Maximum = 32767;
            _left.TickFrequency = 4096;
            _left.SetBounds(12, y + 16, 460, 32);
            _left.AutoSize = false;
            _left.Value = Rest;
            _left.Scroll += (s, e) => { ShowVal(_left, _leftVal); Kick(); };
            Controls.Add(_left);
            _leftVal.SetBounds(480, y + 20, 130, 22);
            Controls.Add(_leftVal);
            ShowVal(_left, _leftVal);

            y += 56;
            AddBtn("Rest 16384", 12, y, (s, e) => { SetBoth(Rest); Kick(true); });
            AddBtn("Slack 0", 120, y, (s, e) => { SetBoth(0); Kick(true); });
            AddBtn("Tug right", 230, y, (s, e) => Tug(1));
            AddBtn("Tug left", 340, y, (s, e) => Tug(2));
            AddBtn("Enable live", 450, y, (s, e) =>
            {
                if (_sp == null || !_sp.IsOpen) { Log("not connected"); return; }
                var t = new Thread(() =>
                {
                    byte[] drive = { 0x00, 0x00, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00, 0xFF };
                    byte[] hwOn = { 0x01, 0xE8, 0x03, 0xC8, 0x01, 0x84, 0x03, 0xB8, 0x0B };
                    byte[] load = { 0x00, 0x12, 0x01, 0x03, 0x1E, 0x00, 0x01, 0x00, 0x00, 0x02 };
                    byte[] ang = { 0x5A, 0x28, 0x32, 0x3C, 0x46, 0x50, 0x5A, 0x64, 0x6E };
                    UiLog("enable live 0x01/0x02");
                    ApplyLive(0x01, drive, hwOn, load, ang);
                    ApplyLive(0x02, drive, hwOn, load, ang);
                    _ready = true;
                    UiLog("enable live done");
                });
                t.IsBackground = true;
                t.Start();
            });

            y += 40;
            _pkt.SetBounds(12, y, 600, 48);
            _pkt.Font = new Font("Consolas", 8);
            Controls.Add(_pkt);

            y += 52;
            _log.Multiline = true;
            _log.ReadOnly = true;
            _log.ScrollBars = ScrollBars.Vertical;
            _log.Font = new Font("Consolas", 8);
            _log.SetBounds(12, y, 600, 250);
            Controls.Add(_log);

            _timer.Interval = 10;
            _timer.Tick += (s, e) =>
            {
                if (_ready && _stream.Checked) Kick(false);
                DrainRx();
            };
            _timer.Start();
            _tug.Interval = 400;
            _tug.Tick += TugDone;
            FormClosing += (s, e) => ClosePort();

            Log("Close Sim Commander before Connect — COM3 is exclusive.");
            Log("Live capture: 921600 8N1, init R/w/C/D/H/L/d, then 64-byte S+O+F9+F8.");
            Log("Rest position in-game was ~16384 (left inverted to -16384).");
        }

        void AddBtn(string text, int x, int y, EventHandler click)
        {
            var b = new Button { Text = text, Left = x, Top = y, Width = 100, Height = 28 };
            b.Click += click;
            Controls.Add(b);
        }

        static Label Lbl(string t, int x, int y)
        {
            return new Label { Text = t, Left = x, Top = y, AutoSize = true };
        }

        static void ShowVal(TrackBar bar, Label lab)
        {
            lab.Text = bar.Value + "  (" + (bar.Value * 100 / 32767) + "%)";
        }

        void SetBoth(int v)
        {
            _right.Value = v;
            _left.Value = v;
            ShowVal(_right, _rightVal);
            ShowVal(_left, _leftVal);
        }

        static byte[] P16(byte addr, byte type, params byte[] payload)
        {
            var p = new byte[16];
            p[0] = Stx;
            p[1] = addr;
            p[2] = type;
            int n = payload.Length;
            if (n > 11) n = 11;
            if (n > 0) Array.Copy(payload, 0, p, 3, n);
            p[14] = Footer;
            p[15] = Etx;
            return p;
        }

        static void PutI16(byte[] p, int o, int v)
        {
            if (v < -32768) v = -32768;
            if (v > 32767) v = 32767;
            p[o] = (byte)(v & 0xFF);
            p[o + 1] = (byte)((v >> 8) & 0xFF);
        }

        byte[] Stream64()
        {
            int r = _right.Value;
            int l = _left.Value;
            if (_invertLeft.Checked) l = -l;
            byte seq = _seq;
            _seq++;
            if (_seq == 0) _seq = 1;

            var p = new byte[64];
            // S
            p[0] = Stx; p[1] = seq; p[2] = 0x53;
            PutI16(p, 3, r);
            PutI16(p, 5, l);
            p[13] = 0x03; p[14] = Footer; p[15] = Etx;
            // O
            p[16] = Stx; p[17] = seq; p[18] = 0x4F;
            p[29] = 0x04; p[30] = Footer; p[31] = Etx;
            // F page 9
            p[32] = Stx; p[33] = 0x09; p[34] = 0x46;
            p[42] = Footer; p[43] = Etx;
            // F page 8
            p[48] = Stx; p[49] = 0x08; p[50] = 0x46;
            p[58] = Footer; p[59] = Etx;
            return p;
        }

        static string Hex(byte[] p, int max)
        {
            int n = p.Length < max ? p.Length : max;
            var sb = new StringBuilder(n * 3);
            for (int i = 0; i < n; i++) sb.Append(p[i].ToString("X2")).Append(' ');
            if (p.Length > max) sb.Append("...");
            return sb.ToString();
        }

        void RefreshPorts()
        {
            var cur = _port.SelectedItem as string;
            _port.Items.Clear();
            foreach (var p in SerialPort.GetPortNames()) _port.Items.Add(p);
            if (cur != null && _port.Items.Contains(cur)) _port.SelectedItem = cur;
            else if (_port.Items.Contains("COM3")) _port.SelectedItem = "COM3";
            else if (_port.Items.Count > 0) _port.SelectedIndex = 0;
            Log("ports: " + string.Join(", ", SerialPort.GetPortNames()));
        }

        void ConnectClick(object sender, EventArgs e)
        {
            if (_sp != null && _sp.IsOpen) { ClosePort(); return; }
            if (_port.SelectedItem == null) { Log("No COM port."); return; }
            if (_initing) { Log("init already running"); return; }
            try
            {
                _sp = new SerialPort((string)_port.SelectedItem, (int)_baud.SelectedItem, Parity.None, 8, StopBits.One);
                _sp.Handshake = Handshake.None;
                _sp.DtrEnable = true;
                _sp.RtsEnable = true;
                _sp.ReadTimeout = 50;
                _sp.WriteTimeout = 500;
                _sp.Open();
                _connect.Text = "Disconnect";
                _txCount = 0;
                _seq = 1;
                _ready = false;
                Log("opened " + _sp.PortName + " @ " + _sp.BaudRate);
                _initing = true;
                var t = new Thread(InitThread);
                t.IsBackground = true;
                t.Start();
            }
            catch (Exception ex)
            {
                Log("open failed: " + ex.Message);
                Log("If access denied, close Sim Commander first.");
                ClosePort();
            }
        }

        void InitThread()
        {
            try
            {
                UiLog("init: reset");
                Send(P16(0x00, 0x52), true);
                Thread.Sleep(20);

                UiLog("init: query interfaces 1-4");
                Send(P16(0x81, 0x77), true); Thread.Sleep(100);
                Send(P16(0x82, 0x77), true); Thread.Sleep(100);
                Send(P16(0x83, 0x77), true); Thread.Sleep(100);
                Send(P16(0x84, 0x77), true); Thread.Sleep(150);

                byte[] drive = { 0x00, 0x00, 0x04, 0x02, 0x00, 0x00, 0x00, 0x00, 0xFF };
                byte[] hwOn = { 0x01, 0xE8, 0x03, 0xC8, 0x01, 0x84, 0x03, 0xB8, 0x0B };
                byte[] hwOff = { 0x00, 0xE8, 0x03, 0xC8, 0x01, 0x84, 0x03, 0xB8, 0x0B };
                byte[] load = { 0x00, 0x12, 0x01, 0x03, 0x1E, 0x00, 0x01, 0x00, 0x00, 0x02 };
                byte[] ang = { 0x5A, 0x28, 0x32, 0x3C, 0x46, 0x50, 0x5A, 0x64, 0x6E };

                UiLog("init: right (port 1) D/H/L/d");
                Send(P16(0x07, 0x43, 0xFF), true); Thread.Sleep(20);
                ConfigPort(0x41, drive, hwOn, load, ang);

                UiLog("init: left (port 2) D/H/L/d");
                Send(P16(0x07, 0x43, 0xFF), true); Thread.Sleep(20);
                ConfigPort(0x42, drive, hwOn, load, ang);

                UiLog("init: calibrate (wait 4.5s for home)");
                Send(P16(0x03, 0x43, 0x02, 0x00, 0x40, 0x64, 0x00, 0x1E, 0x1E, 0xB4, 0x14), true);
                Send(P16(0x05, 0x43, 0x01, 0x00, 0x40, 0x64, 0x00, 0x1E, 0x1E, 0xB4, 0x14), true);
                Send(P16(0x86, 0x43), true);
                Thread.Sleep(4500);

                UiLog("init: post-home config 0x41/0x42 H=00");
                ConfigPort(0x41, drive, hwOff, load, ang);
                ConfigPort(0x42, drive, hwOff, load, ang);

                // Game-start: apply LIVE interface addrs 0x01/0x02 (not 0x41/0x42)
                // order L, d, D, H with hardware-enable=01.
                UiLog("init: enable live ifaces 0x01/0x02 (L/d/D/H=01)");
                ApplyLive(0x01, drive, hwOn, load, ang);
                ApplyLive(0x02, drive, hwOn, load, ang);

                BeginInvoke(new Action(() =>
                {
                    SetBoth(Rest);
                    _ready = true;
                    Kick(true);
                    Log("init done — sliders live. Rest=16384. Tug should move now.");
                }));
            }
            catch (Exception ex)
            {
                UiLog("init failed: " + ex.Message);
            }
            finally
            {
                _initing = false;
            }
        }

        void ConfigPort(byte addr, byte[] drive, byte[] hw, byte[] load, byte[] ang)
        {
            Send(P16(addr, 0x44, drive), true); Thread.Sleep(10);
            Send(P16(addr, 0x48, hw), true); Thread.Sleep(10);
            Send(P16(addr, 0x4C, load), true); Thread.Sleep(10);
            Send(P16(addr, 0x64, ang), true); Thread.Sleep(15);
        }

        void ApplyLive(byte addr, byte[] drive, byte[] hw, byte[] load, byte[] ang)
        {
            Send(P16(addr, 0x4C, load), true); Thread.Sleep(8);
            Send(P16(addr, 0x64, ang), true); Thread.Sleep(8);
            Send(P16(addr, 0x44, drive), true); Thread.Sleep(8);
            Send(P16(addr, 0x48, hw), true); Thread.Sleep(10);
        }

        void Kick(bool log)
        {
            if (_sp == null || !_sp.IsOpen || !_ready) return;
            Send(Stream64(), log);
        }

        void Kick()
        {
            Kick(false);
        }

        void Tug(int which)
        {
            _tug.Stop();
            _tugWhich = which;
            var bar = which == 1 ? _right : _left;
            _tugSaved = bar.Value;
            int v = _tugSaved + 4000;
            if (v > 32767) v = 32767;
            bar.Value = v;
            ShowVal(bar, which == 1 ? _rightVal : _leftVal);
            Kick(true);
            Log("tug " + (which == 1 ? "right" : "left") + " " + _tugSaved + " -> " + v);
            _tug.Start();
        }

        void TugDone(object sender, EventArgs e)
        {
            _tug.Stop();
            var bar = _tugWhich == 1 ? _right : _left;
            var lab = _tugWhich == 1 ? _rightVal : _leftVal;
            if (_tugSaved < bar.Minimum) _tugSaved = bar.Minimum;
            if (_tugSaved > bar.Maximum) _tugSaved = bar.Maximum;
            bar.Value = _tugSaved;
            ShowVal(bar, lab);
            Kick(true);
            _tugWhich = 0;
        }

        void Send(byte[] p, bool log)
        {
            _pkt.Text = Hex(p, 64);
            if (_sp == null || !_sp.IsOpen) return;
            lock (_io)
            {
                try
                {
                    _sp.Write(p, 0, p.Length);
                    _txCount++;
                    UiTx();
                    if (log) UiLog("TX " + p.Length + "B " + Hex(p, 32));
                }
                catch (Exception ex)
                {
                    UiLog("write failed: " + ex.Message);
                }
            }
        }

        void DrainRx()
        {
            if (_sp == null || !_sp.IsOpen) return;
            lock (_io)
            {
                try
                {
                    int n = _sp.BytesToRead;
                    if (n <= 0) return;
                    var buf = new byte[Math.Min(n, 256)];
                    int r = _sp.Read(buf, 0, buf.Length);
                    if (r > 0) UiLog("RX " + r + "B " + Hex(Slice(buf, r), 48));
                }
                catch { }
            }
        }

        static byte[] Slice(byte[] b, int n)
        {
            var o = new byte[n];
            Array.Copy(b, o, n);
            return o;
        }

        void ClosePort()
        {
            _ready = false;
            _initing = false;
            try
            {
                if (_sp != null && _sp.IsOpen)
                {
                    try
                    {
                        SetBoth(Rest);
                        _sp.Write(Stream64(), 0, 64);
                    }
                    catch { }
                    _sp.Close();
                }
            }
            catch { }
            _sp = null;
            _connect.Text = "Connect + init";
            _tx.Text = "TX " + _txCount + " (disconnected)";
        }

        void UiLog(string s)
        {
            if (InvokeRequired) { BeginInvoke(new Action<string>(UiLog), s); return; }
            Log(s);
        }

        void UiTx()
        {
            if (InvokeRequired) { BeginInvoke(new Action(UiTx)); return; }
            _tx.Text = "TX " + _txCount;
        }

        void Log(string s)
        {
            if (_log.TextLength > 40000) _log.Clear();
            _log.AppendText(DateTime.Now.ToString("HH:mm:ss.fff ", CultureInfo.InvariantCulture) + s + "\r\n");
        }
    }
}
