using System.ComponentModel;
using Windows.Media;

namespace DesktopRadio;

internal sealed class RadioWindow : Form
{
    private readonly Catalog catalog = new();
    private readonly PreferencesStore store;
    private readonly Preferences preferences;
    private readonly RadioPlayer player = new();
    private readonly System.Windows.Forms.Timer meterTimer = new() { Interval = 60 };
    private readonly System.Windows.Forms.Timer introTimer = new() { Interval = 420 };
    private readonly System.Windows.Forms.Timer saveTimer = new() { Interval = 400 };
    private readonly ToolTip tools = new();
    private readonly CancellationTokenSource lifetime = new();
    private readonly bool diagnostic;
    private readonly NotifyIcon? tray;
    private readonly ContextMenuStrip? trayMenu;
    private readonly MediaControls? mediaControls;
    private readonly Action? quit;
    private Palette colors = Palette.Graphite;
    private Labels labels;
    private string page = "radio";
    private string query = "";
    private Station? current;
    private Playback playback = new(PlayState.Ready);
    private int playbackRevision;
    private bool revealStation;
    private bool settingsDirty;
    private bool stationListDirty;
    private bool disposing;
    private bool checkInProgress;
    private int? downloadProgress;
    private WindowsUpdate? update;
    private string updateStatus = "";
    private Label? updateLabel;
    private Button? updateButton;
    private Button? checkButton;
    private StationList? stations;
    private Label? stationName;
    private Label? stationDetail;
    private Label? stateLabel;
    private Button? star;
    private Button? playButton;
    private Visualization? meter;
    private Panel? introOverlay;
    private TextBox? searchField;
    public int PaintCount { get; private set; }
    protected override bool ShowWithoutActivation => diagnostic;

    public RadioWindow(PreferencesStore store, bool diagnostic, Action? quit = null)
    {
        this.store = store;
        this.diagnostic = diagnostic;
        this.quit = quit;
        preferences = diagnostic ? new() : store.Load();
        labels = new(preferences.Language);
        if (diagnostic)
        {
            preferences.StationId = "radio-4you";
            preferences.Favorites.UnionWith(["radio-4you", "radius"]);
            preferences.EqualizerEnabled = true;
            preferences.Equalizer = [3, 4, 2, 0, -1, 0, 1, 2, 2, 1];
        }

        current = catalog.Stations.FirstOrDefault(station => station.Id == preferences.StationId);
        Text = "Desktop Radio";
        AutoScaleMode = AutoScaleMode.Dpi;
        Font = new("Segoe UI", 10f);
        ClientSize = new(460, 590);
        FormBorderStyle = FormBorderStyle.None;
        StartPosition = FormStartPosition.Manual;
        ShowInTaskbar = false;
        KeyPreview = true;
        Icon = Icon.ExtractAssociatedIcon(Environment.ProcessPath!) ?? SystemIcons.Application;
        DoubleBuffered = true;
        _ = Handle;
        if (!diagnostic)
        {
            trayMenu = new ContextMenuStrip();
            tray = new NotifyIcon { Icon = TrayIcon(), Text = Text, ContextMenuStrip = trayMenu, Visible = true };
            tray.MouseClick += (_, e) => { if (e.Button == MouseButtons.Left) TogglePanel(Cursor.Position); };
            RefreshTrayMenu();
            mediaControls = MediaControls.Attach(Handle, HandleMediaButton);
        }
        meterTimer.Tick += (_, _) => meter?.Read();
        introTimer.Tick += (_, _) => EndIntro();
        saveTimer.Tick += (_, _) => SaveNow();
        FormClosing += (_, args) =>
        {
            if (!disposing && args.CloseReason == CloseReason.UserClosing)
            {
                args.Cancel = true;
                Hide();
            }
        };
        VisibleChanged += (_, _) =>
        {
            if (!Visible) EndIntro();
            RefreshMeter();
        };
        player.SetMixer(preferences);
        Rebuild();
    }

    private static Icon TrayIcon()
    {
        var module = GetModuleHandle(null);
        if (LoadIconMetric(module, (nint)32512, 0, out var handle) != 0 || handle == 0)
            return Icon.ExtractAssociatedIcon(Environment.ProcessPath!) ?? SystemIcons.Application;
        using var icon = Icon.FromHandle(handle);
        var copy = (Icon)icon.Clone();
        DestroyIcon(handle);
        return copy;
    }

    public void Start()
    {
        if (current is not null && preferences.PlayOnStartup) Tune(current);
        if (preferences.CheckUpdatesOnStartup) _ = CheckUpdates(manual: false);
    }

    public void ShowPanel(Point? anchor = null, bool intro = true)
    {
        if (Visible) { Activate(); return; }
        var area = Screen.FromPoint(anchor ?? Cursor.Position).WorkingArea;
        var margin = Math.Max(8, DeviceDpi / 12);
        Location = new(Math.Max(area.Left, area.Right - Width - margin), Math.Max(area.Top, area.Bottom - Height - margin));
        Show();
        Activate();
        if (stationListDirty) RefreshStations();
        RefreshCurrent();
        if (intro && preferences.ShowIntro && page == "radio") BeginIntro();
        RevealPlayingStation();
    }

    private void TogglePanel(Point point)
    {
        if (!Visible) ShowPanel(point);
        else Hide();
    }

    private void HandleMediaButton(SystemMediaTransportControlsButton button)
    {
        if (disposing) return;
        try
        {
            BeginInvoke(() =>
            {
                if (disposing) return;
                switch (button)
                {
                    case SystemMediaTransportControlsButton.Play:
                        if (playback.State is not (PlayState.Playing or PlayState.Connecting)) Toggle();
                        break;
                    case SystemMediaTransportControlsButton.Pause:
                    case SystemMediaTransportControlsButton.Stop:
                        if (playback.State is PlayState.Playing or PlayState.Connecting) Toggle();
                        break;
                    case SystemMediaTransportControlsButton.Next:
                        Navigate(1); break;
                    case SystemMediaTransportControlsButton.Previous:
                        Navigate(-1); break;
                }
            });
        }
        catch (InvalidOperationException) { }
    }

    private void RefreshTrayMenu()
    {
        if (trayMenu is null) return;
        foreach (var item in trayMenu.Items.Cast<ToolStripItem>().ToArray()) item.Dispose();
        trayMenu.Items.Clear();
        trayMenu.Items.Add(labels["Open radio"], null, (_, _) => ShowPanel());
        trayMenu.Items.Add(labels["Previous station"], null, (_, _) => Navigate(-1));
        trayMenu.Items.Add(labels["Next station"], null, (_, _) => Navigate(1));
        trayMenu.Items.Add(new ToolStripSeparator());
        trayMenu.Items.Add(labels["Settings"], null, (_, _) => { Open("settings"); ShowPanel(intro: false); });
        trayMenu.Items.Add(labels["Quit"], null, (_, _) => quit?.Invoke());
    }

    protected override bool ProcessCmdKey(ref Message message, Keys keys)
    {
        if (introOverlay is not null) EndIntro();
        switch (keys)
        {
            case Keys.Escape:
                if (page == "radio") Hide(); else Open("radio");
                return true;
            case Keys.Control | Keys.F:
                if (page != "radio") Open("radio");
                searchField?.Focus();
                return true;
            case Keys.MediaPlayPause:
                Toggle(); return true;
            case Keys.MediaNextTrack:
                Navigate(1); return true;
            case Keys.MediaPreviousTrack:
                Navigate(-1); return true;
            default: return base.ProcessCmdKey(ref message, keys);
        }
    }

    private void BeginIntro()
    {
        if (introOverlay is not null) return;
        var cover = new Panel { Dock = DockStyle.Fill, BackColor = colors.Window, Cursor = Cursors.Hand };
        var logo = new Panel { Size = new(126, 84), Anchor = AnchorStyles.None };
        logo.Paint += (_, e) =>
        {
            e.Graphics.DrawIcon(Icon!, new Rectangle(43, 0, 40, 40));
            using var font = new Font(Font, FontStyle.Bold);
            TextRenderer.DrawText(e.Graphics, "Desktop Radio", font,
                new Rectangle(0, 46, 126, 30), colors.Text, TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter);
        };
        cover.Controls.Add(logo);
        cover.Resize += (_, _) => logo.Location = new((cover.Width - logo.Width) / 2, (cover.Height - logo.Height) / 2);
        cover.Click += (_, _) => EndIntro();
        logo.Click += (_, _) => EndIntro();
        Controls.Add(cover);
        cover.BringToFront();
        introOverlay = cover;
        introTimer.Start();
    }

    private void EndIntro()
    {
        introTimer.Stop();
        introOverlay?.Dispose();
        introOverlay = null;
    }

    private void Open(string destination)
    {
        if (page == destination) return;
        page = destination;
        revealStation = true;
        Rebuild();
    }

    private void Rebuild()
    {
        EndIntro();
        colors = preferences.Theme == "amber" ? Palette.Amber : Palette.Graphite;
        labels = new(preferences.Language);
        BackColor = colors.Window;
        ForeColor = colors.Text;
        SuspendLayout();
        try
        {
            foreach (Control child in Controls.Cast<Control>().ToArray()) child.Dispose();
            stations = null;
            searchField = null;
            meter = null;
            stationName = stationDetail = stateLabel = updateLabel = null;
            playButton = star = checkButton = updateButton = null;
            var layout = Column(54, 0);
            layout.Padding = new Padding(18, 4, 18, 12);
            layout.Controls.Add(Header(), 0, 0);
            layout.Controls.Add(page switch
            {
                "settings" => Settings(), "stations" => StationSettings(), "equalizer" => Equalizer(), _ => Radio()
            }, 0, 1);
            Controls.Add(layout);
            RefreshCurrent();
            RefreshUpdate();
            RefreshMeter();
        }
        finally { ResumeLayout(performLayout: true); }
    }

    private Control Header()
    {
        var title = page switch
        {
            "settings" => labels["Settings"], "stations" => labels["Manage stations"],
            "equalizer" => labels["Equalizer"], _ => "Desktop Radio"
        };
        var grid = Row(30, 0, page == "radio" ? 45 : 0, page == "radio" ? 46 : 0, 37);
        var mark = new Panel { Dock = DockStyle.Fill, Margin = new Padding(0, 0, 5, 0) };
        mark.Paint += (_, args) => args.Graphics.DrawIcon(Icon!, new Rectangle(0, (mark.Height - 23) / 2, 23, 23));
        grid.Controls.Add(mark, 0, 0);
        grid.Controls.Add(Caption(title, 16, colors.Text, bold: true), 1, 0);
        if (page == "radio")
        {
            grid.Controls.Add(Button("≋", labels["Equalizer"], () => Open("equalizer"), bold: true), 2, 0);
            grid.Controls.Add(Button("⚙", labels["Settings"], () => Open("settings")), 3, 0);
        }
        else
        {
            grid.SetColumnSpan(grid.GetControlFromPosition(1, 0)!, 3);
        }
        grid.Controls.Add(Button(page == "radio" ? "×" : "←", page == "radio" ? labels["Done"] : labels["Radio"],
            () => { if (page == "radio") Hide(); else Open(page == "stations" ? "settings" : "radio"); }), 4, 0);
        return grid;
    }

    private Control Radio()
    {
        var groups = VisibleGroups();
        if (!groups.Contains(preferences.Group)) preferences.Group = groups[0];
        var layout = Column(140, 11, 38, 43, 0, 48);
        var hero = Column(44, 25, 43);
        hero.BackColor = colors.Surface;
        hero.Padding = new Padding(14, 6, 14, 0);
        var titleRow = Row(0, 42);
        stationName = Caption("", 18, colors.Text, bold: true);
        titleRow.Controls.Add(stationName, 0, 0);
        star = Button("☆", labels["Add to favorites"], Favorite);
        titleRow.Controls.Add(star, 1, 0);
        hero.Controls.Add(titleRow, 0, 0);
        stationDetail = Caption("", 9, colors.Muted);
        hero.Controls.Add(stationDetail, 0, 1);
        meter = new(player.Meter) { Dock = DockStyle.Fill, Mode = preferences.Visualization,
            Smooth = preferences.SmoothVisualization, Accent = colors.Accent, Grid = colors.Border, BackColor = colors.Surface,
            AccessibleName = labels["Visualization"] };
        hero.Controls.Add(meter, 0, 2);
        layout.Controls.Add(hero, 0, 0);

        var regionGrid = Row(groups.Select(_ => 0).ToArray());
        regionGrid.Tag = "regions";
        for (var i = 0; i < groups.Length; i++)
        {
            var group = groups[i];
            var button = Button(labels.Group(group), labels.Group(group), () => SelectGroup(group), bold: true);
            button.Font = new(Font.FontFamily, preferences.Language == "en" ? 8.7f : 8.4f, FontStyle.Bold);
            button.Tag = group;
            regionGrid.Controls.Add(button, i, 0);
        }
        TintGroups(regionGrid);
        layout.Controls.Add(regionGrid, 0, 2);

        var search = new Panel { BackColor = colors.Surface, Dock = DockStyle.Fill, Padding = new Padding(11, 8, 11, 0),
            Margin = new Padding(0, 5, 0, 7) };
        searchField = new SearchField(labels["Search by name or frequency"], colors.Muted)
        { Dock = DockStyle.Fill, BorderStyle = BorderStyle.None, Font = Font, BackColor = colors.Surface,
            ForeColor = colors.Text,
            AccessibleName = labels["Search by name or frequency"], Text = query };
        searchField.TextChanged += (_, _) => { query = searchField.Text; RefreshStations(); };
        searchField.KeyDown += (_, e) =>
        {
            if (stations?.Items.Count > 0 && e.KeyCode == Keys.Down)
            {
                stations.Focus();
                if (stations.SelectedIndex < 0) stations.SelectedIndex = 0;
                e.SuppressKeyPress = true;
            }
        };
        search.Controls.Add(searchField);
        layout.Controls.Add(search, 0, 3);

        stations = new StationList(colors, preferences.Language) { Dock = DockStyle.Fill, Margin = Padding.Empty,
            AccessibleName = labels["Radio"] };
        stations.MouseClick += (_, e) =>
        {
            var index = stations.IndexFromPoint(e.Location);
            if (e.Button == MouseButtons.Left && index >= 0) Tune((Station)stations.Items[index]);
        };
        stations.KeyDown += (_, e) =>
        {
            if (e.KeyCode != Keys.Enter || stations.SelectedItem is not Station selected) return;
            Tune(selected);
            e.SuppressKeyPress = true;
        };
        layout.Controls.Add(stations, 0, 4);

        var footer = Row(46, 52, 46, 0, 105);
        footer.Padding = new Padding(0, 5, 0, 0);
        footer.Controls.Add(Button("⏮", labels["Previous station"], () => Navigate(-1)), 0, 0);
        playButton = Button("▶", labels["Play"], Toggle, primary: true);
        footer.Controls.Add(playButton, 1, 0);
        footer.Controls.Add(Button("⏭", labels["Next station"], () => Navigate(1)), 2, 0);
        stateLabel = Caption("", 8.7f, colors.Muted, padding: 6);
        footer.Controls.Add(stateLabel, 3, 0);
        var volume = Slider(Math.Clamp(preferences.Volume, 0, 100), 0, 100, labels["Volume"], value =>
        {
            preferences.Volume = value; ApplyMixer();
        });
        volume.TickStyle = TickStyle.None;
        volume.Margin = new Padding(3, 10, 0, 4);
        footer.Controls.Add(volume, 4, 0);
        layout.Controls.Add(footer, 0, 5);
        RefreshStations();
        return layout;
    }

    private Control Settings()
    {
        var layout = Column(34, 37, 37, 37, 31, 31, 31, 35, 11, 36, 35, 40, 35, 0);
        layout.Controls.Add(Caption("Desktop Radio   v" + Updates.Current, 10, colors.Muted), 0, 0);
        layout.Controls.Add(Option("Language", ["English", "Русский", "Қазақша"], ["en", "ru", "kk"], preferences.Language,
            value => { preferences.Language = value; SaveSoon(); Rebuild(); RefreshTrayMenu(); }), 0, 1);
        layout.Controls.Add(Option("Theme", ["Graphite", "Amber"], ["graphite", "amber"], preferences.Theme,
            value => { preferences.Theme = value; SaveSoon(); Rebuild(); }), 0, 2);
        layout.Controls.Add(Option("Visualization", ["Spectrum", "Waveform", "Off"], ["spectrum", "waveform", "off"],
            preferences.Visualization, value => { preferences.Visualization = value; SaveSoon(); }), 0, 3);
        layout.Controls.Add(Check("Smooth visualization", preferences.SmoothVisualization, value =>
        { preferences.SmoothVisualization = value; SaveSoon(); }), 0, 4);
        layout.Controls.Add(Check("Show intro when opening the panel", preferences.ShowIntro, value =>
        { preferences.ShowIntro = value; SaveSoon(); }), 0, 5);
        layout.Controls.Add(Check("Resume last station on startup", preferences.PlayOnStartup, value =>
        { preferences.PlayOnStartup = value; SaveSoon(); }), 0, 6);
        layout.Controls.Add(Button(labels["Manage stations"], labels["Manage stations"], () => Open("stations")), 0, 7);
        layout.Controls.Add(Check("Check for updates on startup", preferences.CheckUpdatesOnStartup, value =>
        { preferences.CheckUpdatesOnStartup = value; SaveSoon(); }), 0, 9);
        checkButton = Button(labels["Check for updates"], labels["Check for updates"], () => _ = CheckUpdates(manual: true));
        layout.Controls.Add(checkButton, 0, 10);
        updateLabel = Caption("", 9, colors.Muted);
        updateLabel.AutoEllipsis = false;
        layout.Controls.Add(updateLabel, 0, 11);
        updateButton = Button(labels["Download and install"], labels["Download and install"], () => _ = InstallUpdate());
        layout.Controls.Add(updateButton, 0, 12);
        return layout;
    }

    private Control StationSettings()
    {
        var layout = Column(30, 34, 35, 35, 0, 34, 38, 41);
        var visible = Row(0, 0, 0, 0, 0);
        var group = preferences.Group;
        var list = new StationList(colors, preferences.Language) { Dock = DockStyle.Fill, AccessibleName = labels["Manage stations"] };
        var groupChoice = Choice(Catalog.Groups.Select(labels.Group).ToArray());
        groupChoice.AccessibleName = labels["Manage stations"];
        var selectedVisible = Check("Station is visible", false, value =>
        {
            if (list.SelectedItem is not Station station) return;
            if (value) preferences.HiddenStations.Remove(station.Id); else preferences.HiddenStations.Add(station.Id);
            SaveSoon();
        });
        void Fill(string? selected = null)
        {
            list.Items.Clear();
            foreach (var station in catalog.InGroup(group, preferences, includeHidden: true)) list.Items.Add(station);
            list.EmptyMessage = labels[group == Catalog.Favorites ? "No favorites yet" : "No visible stations"];
            list.SelectedIndex = selected is null ? Math.Min(0, list.Items.Count - 1) :
                Enumerable.Range(0, list.Items.Count).FirstOrDefault(index => ((Station)list.Items[index]).Id == selected, -1);
        }
        void Move(int offset)
        {
            if (list.SelectedItem is not Station station) return;
            var from = list.SelectedIndex;
            var to = from + offset;
            if (to < 0 || to >= list.Items.Count) return;
            var order = list.Items.Cast<Station>().Select(item => item.Id).ToList();
            (order[from], order[to]) = (order[to], order[from]);
            preferences.Order[group] = order;
            SaveSoon();
            Fill(station.Id);
        }
        for (var i = 0; i < Catalog.Groups.Length; i++)
        {
            var item = Catalog.Groups[i];
            var button = Button(labels.Group(item), labels.Group(item), () =>
            {
                if (!preferences.HiddenGroups.Remove(item) && preferences.HiddenGroups.Count < Catalog.Groups.Length - 1)
                    preferences.HiddenGroups.Add(item);
                SaveSoon();
                foreach (Button tab in visible.Controls) ApplyButton(tab, !preferences.HiddenGroups.Contains((string)tab.Tag!));
            });
            button.Tag = item;
            button.Font = new(Font.FontFamily, 8.2f);
            ApplyButton(button, !preferences.HiddenGroups.Contains(item));
            visible.Controls.Add(button, i, 0);
        }
        groupChoice.SelectedIndex = Math.Max(0, Array.IndexOf(Catalog.Groups, group));
        groupChoice.SelectedIndexChanged += (_, _) => { group = Catalog.Groups[groupChoice.SelectedIndex]; Fill(); };
        list.SelectedIndexChanged += (_, _) =>
        {
            selectedVisible.Enabled = list.SelectedItem is Station;
            if (list.SelectedItem is Station selected) selectedVisible.Checked = !preferences.HiddenStations.Contains(selected.Id);
        };
        var movement = Row(0, 0);
        movement.Controls.Add(Button(labels["Move up"], labels["Move up"], () => Move(-1)), 0, 0);
        movement.Controls.Add(Button(labels["Move down"], labels["Move down"], () => Move(1)), 1, 0);
        layout.Controls.Add(Caption(labels["Visible groups"], 10, colors.Muted), 0, 0);
        layout.Controls.Add(visible, 0, 1);
        layout.Controls.Add(Check("Hide recently unavailable stations", preferences.HideUnavailable, value =>
        { preferences.HideUnavailable = value; SaveSoon(); }), 0, 2);
        layout.Controls.Add(groupChoice, 0, 3);
        layout.Controls.Add(list, 0, 4);
        layout.Controls.Add(selectedVisible, 0, 5);
        layout.Controls.Add(movement, 0, 6);
        layout.Controls.Add(Button(labels["Done"], labels["Done"], () => Open("settings")), 0, 7);
        Fill();
        return layout;
    }

    private Control Equalizer()
    {
        var panel = Column(42, 32, 280, 36, 31, 38, 0);
        panel.Controls.Add(Check("Enable equalizer", preferences.EqualizerEnabled, value =>
        { preferences.EqualizerEnabled = value; ApplyMixer(); }), 0, 0);
        panel.Controls.Add(Caption("+12 dB", 9, colors.Muted), 0, 1);
        var bands = Row(EqualizerProcessor.Frequencies.Select(_ => 0).ToArray());
        var captions = Row(EqualizerProcessor.Frequencies.Select(_ => 0).ToArray());
        var readings = Row(EqualizerProcessor.Frequencies.Select(_ => 0).ToArray());
        var sliders = new TrackBar[EqualizerProcessor.Frequencies.Length];
        for (var i = 0; i < sliders.Length; i++)
        {
            var index = i;
            var frequency = EqualizerProcessor.Frequencies[i];
            var reading = Caption(GainText(preferences.Equalizer[i]), 9, colors.Accent, center: true);
            var slider = Slider(Math.Clamp(preferences.Equalizer[i], -12, 12), -12, 12, $"{labels["Band"]} {frequency} Hz", value =>
            {
                preferences.Equalizer[index] = value;
                reading.Text = GainText(value);
                ApplyMixer();
            });
            slider.Orientation = Orientation.Vertical;
            slider.TickFrequency = 6;
            slider.TickStyle = TickStyle.Both;
            slider.Margin = Padding.Empty;
            bands.Controls.Add(slider, i, 0);
            sliders[i] = slider;
            captions.Controls.Add(Caption(frequency >= 1000 ? $"{frequency / 1000}k" : $"{frequency}", 9, colors.Text, center: true), i, 0);
            readings.Controls.Add(reading, i, 0);
        }
        panel.Controls.Add(bands, 0, 2);
        panel.Controls.Add(captions, 0, 3);
        panel.Controls.Add(readings, 0, 4);
        panel.Controls.Add(Button(labels["Reset bands"], labels["Reset bands"], () =>
        {
            foreach (var slider in sliders) slider.Value = 0;
        }), 0, 5);
        return panel;
    }

    private static string GainText(int value) => value.ToString("+0;-0;0");

    private void ApplyMixer() { player.SetMixer(preferences); SaveSoon(); }

    private string[] VisibleGroups()
    {
        var groups = Catalog.Groups.Where(group => !preferences.HiddenGroups.Contains(group)).ToArray();
        return groups.Length == 0 ? [Catalog.Groups[0]] : groups;
    }

    private void SelectGroup(string group)
    {
        preferences.Group = group;
        SaveSoon();
        if (searchField is not null) searchField.Text = "";
        foreach (var table in AllControls(this).OfType<TableLayoutPanel>().Where(table => Equals(table.Tag, "regions")))
            TintGroups(table);
        RefreshStations();
    }

    private void TintGroups(TableLayoutPanel table)
    {
        foreach (Button button in table.Controls) ApplyButton(button, Equals(button.Tag, preferences.Group));
    }

    private void RefreshStations()
    {
        if (stations is null) return;
        stationListDirty = false;
        var selection = (stations.SelectedItem as Station)?.Id;
        var top = stations.TopIndex;
        var contents = string.IsNullOrWhiteSpace(query) ? catalog.InGroup(preferences.Group, preferences) :
            catalog.Find(query, preferences);
        stations.BeginUpdate();
        stations.Items.Clear();
        stations.Items.AddRange(contents.Cast<object>().ToArray());
        stations.EmptyMessage = labels[!string.IsNullOrWhiteSpace(query) ? "No matching stations" :
            preferences.Group == Catalog.Favorites ? "No favorites yet" : "No visible stations"];
        stations.SelectedIndex = selection is null ? -1 : Enumerable.Range(0, stations.Items.Count)
            .FirstOrDefault(i => ((Station)stations.Items[i]).Id == selection, -1);
        if (top >= 0 && top < stations.Items.Count) stations.TopIndex = top;
        stations.EndUpdate();
        RevealPlayingStation();
        stations.Invalidate();
    }

    private void RevealPlayingStation()
    {
        if (!revealStation || stations is null || current is null) return;
        var index = stations.Items.IndexOf(current);
        if (index < 0) return;
        revealStation = false;
        stations.SelectedIndex = index;
        if (index < stations.TopIndex || index >= stations.TopIndex + Math.Max(1, stations.Height / stations.ItemHeight))
            stations.TopIndex = index;
    }

    private IEnumerable<Station> NavigationList() => string.IsNullOrWhiteSpace(query)
        ? catalog.InGroup(preferences.Group, preferences) : catalog.Find(query, preferences);

    private void Navigate(int offset)
    {
        var list = NavigationList().ToArray();
        if (list.Length == 0) return;
        var index = Array.IndexOf(list, current);
        if (index < 0) index = offset > 0 ? -1 : 0;
        Tune(list[(index + offset + list.Length) % list.Length]);
    }

    private void Toggle()
    {
        if (playback.State is PlayState.Playing or PlayState.Connecting)
        {
            playbackRevision++;
            player.Stop();
            playback = new(PlayState.Ready);
            RefreshCurrent();
        }
        else if ((current ?? NavigationList().FirstOrDefault()) is Station station) Tune(station);
    }

    private void Tune(Station station)
    {
        if (diagnostic) return;
        current = station;
        preferences.StationId = station.Id;
        SaveSoon();
        revealStation = true;
        var revision = ++playbackRevision;
        playback = new(PlayState.Connecting);
        RefreshCurrent();
        player.Start(station, status =>
        {
            if (disposing) return;
            try { BeginInvoke(() => AcceptPlayback(revision, station, status)); }
            catch (InvalidOperationException) { }
        });
    }

    private void AcceptPlayback(int revision, Station station, Playback status)
    {
        if (disposing || revision != playbackRevision) return;
        playback = status;
        if (status.State == PlayState.Unavailable)
        {
            preferences.Unavailable[station.Id] = DateTimeOffset.UtcNow;
            SaveSoon();
            if (preferences.HideUnavailable) StationAvailabilityChanged();
        }
        else if (status.State == PlayState.Playing && preferences.Unavailable.Remove(station.Id))
        {
            SaveSoon();
            if (preferences.HideUnavailable) StationAvailabilityChanged();
        }
        RefreshCurrent();
    }

    private void StationAvailabilityChanged()
    {
        if (Visible) RefreshStations(); else stationListDirty = true;
    }

    private void Favorite()
    {
        if (current is null) return;
        if (!preferences.Favorites.Remove(current.Id)) preferences.Favorites.Add(current.Id);
        SaveSoon();
        if (preferences.Group == Catalog.Favorites) RefreshStations();
        RefreshCurrent();
    }

    private void RefreshCurrent()
    {
        var status = playback.State switch
        {
            PlayState.Connecting => "Connecting…",
            PlayState.Playing => "On air",
            PlayState.Unavailable => "Station unavailable. Try again.",
            PlayState.AudioError => "Audio output failed. Check your sound device.",
            PlayState.EngineMissing => "Install VLC 3 (64-bit) to listen.",
            _ => "Ready to play"
        };
        if (tray is not null)
        {
            var tooltip = current is null ? Text : $"{Text} · {current.Name}";
            if (tray.Text != tooltip) tray.Text = tooltip.Length > 127 ? tooltip[..127] : tooltip;
        }
        if (!Visible && !diagnostic)
        {
            mediaControls?.Update(current, playback);
            return;
        }
        if (stateLabel is not null)
        {
            if (stationName is not null) stationName.Text = current?.Name ?? labels["Choose a station"];
            var detail = current?.Detail(preferences.Language) ?? "";
            if (stationDetail is not null)
            {
                stationDetail.Text = playback.State is PlayState.AudioError or PlayState.EngineMissing
                    ? labels[status] : playback.Track ?? detail;
                tools.SetToolTip(stationDetail, stationDetail.Text);
            }
            stateLabel.Text = playback.State is PlayState.AudioError or PlayState.EngineMissing ? "" : labels[status];
            stateLabel.ForeColor = playback.State is PlayState.Playing ? colors.Accent : colors.Muted;
            tools.SetToolTip(stateLabel, labels[status]);
            if (playButton is not null)
            {
                var active = playback.State is PlayState.Playing or PlayState.Connecting;
                playButton.Text = active ? "■" : "▶";
                NameButton(playButton, labels[active ? "Stop" : "Play"]);
            }
            if (star is not null)
            {
                var saved = current is not null && preferences.Favorites.Contains(current.Id);
                star.Enabled = current is not null;
                star.Text = saved ? "★" : "☆";
                star.ForeColor = saved ? colors.Accent : colors.Text;
                NameButton(star, labels[saved ? "Remove from favorites" : "Add to favorites"]);
            }
            if (stations is not null && stations.PlayingId != (playback.State is PlayState.Playing or PlayState.Connecting ? current?.Id : null))
            {
                stations.PlayingId = playback.State is PlayState.Playing or PlayState.Connecting ? current?.Id : null;
                stations.Invalidate();
            }
            RevealPlayingStation();
        }
        mediaControls?.Update(current, playback);
        RefreshMeter();
    }

    private void RefreshMeter()
    {
        var active = !diagnostic && Visible && page == "radio" && preferences.Visualization != "off" &&
            playback.State == PlayState.Playing;
        player.SetVisualization(active);
        meterTimer.Enabled = active;
        if (!active && !diagnostic && Visible) meter?.Clear();
    }

    private async Task CheckUpdates(bool manual)
    {
        if (checkInProgress || downloadProgress is not null) return;
        checkInProgress = true;
        if (manual) updateStatus = "Checking for updates…";
        RefreshUpdate();
        try
        {
            var result = await Updates.Check(lifetime.Token);
            update = result.Update;
            updateStatus = manual || update is not null ? result.Message : "";
            if (!manual && update is not null)
                tray?.ShowBalloonTip(4500, Text, $"{labels["Update available"]} · v{update.Version}", ToolTipIcon.Info);
        }
        catch when (!disposing) { updateStatus = manual ? "Could not check for updates." : ""; }
        catch (OperationCanceledException) { }
        finally { checkInProgress = false; if (!disposing) RefreshUpdate(); }
    }

    private async Task InstallUpdate()
    {
        if (update is null || downloadProgress is not null) return;
        if (!Updates.IsPortable)
        {
            updateStatus = "Use the portable build to install updates.";
            RefreshUpdate();
            return;
        }
        downloadProgress = 0;
        RefreshUpdate();
        try
        {
            await Updates.Stage(update, new Progress<int>(value =>
            {
                if (downloadProgress is null || disposing) return;
                downloadProgress = value;
                RefreshUpdate();
            }), lifetime.Token);
            updateStatus = "Installing…";
            downloadProgress = null;
            RefreshUpdate();
            quit?.Invoke();
        }
        catch when (!disposing)
        {
            downloadProgress = null;
            updateStatus = "Could not install the update.";
            RefreshUpdate();
        }
        catch (OperationCanceledException) { }
    }

    public void ShowUpdateError()
    {
        updateStatus = "Could not install the update.";
        Open("settings");
        ShowPanel(intro: false);
    }

    private void RefreshUpdate()
    {
        if (checkButton is not null) checkButton.Enabled = !checkInProgress && downloadProgress is null;
        if (updateButton is not null)
        {
            updateButton.Visible = update is not null;
            updateButton.Enabled = downloadProgress is null && !checkInProgress;
        }
        if (updateLabel is not null)
            updateLabel.Text = downloadProgress is int progress ? $"{labels["Downloading…"]} {progress}%" :
                updateStatus == "Update available" && update is not null ? $"{labels[updateStatus]} · v{update.Version}" : labels[updateStatus];
    }

    private void SaveSoon()
    {
        if (diagnostic || disposing) return;
        settingsDirty = true;
        saveTimer.Stop();
        saveTimer.Start();
    }

    private void SaveNow()
    {
        saveTimer.Stop();
        if (!settingsDirty) return;
        try { store.Save(preferences); settingsDirty = false; }
        catch (Exception failure) when (failure is IOException or UnauthorizedAccessException)
        {
            tray?.ShowBalloonTip(3000, Text, labels["Could not save settings."], ToolTipIcon.Warning);
        }
    }

    public async Task Shutdown()
    {
        disposing = true;
        Hide();
        if (tray is not null) tray.Visible = false;
        SaveNow();
        lifetime.Cancel();
        player.Stop();
        await Task.WhenAny(player.Completion, Task.Delay(4500));
    }

    public void CaptureScreen(string filename, string screen)
    {
        playback = new(PlayState.Playing, "Midnight City — M83");
        Open(screen);
        RefreshCurrent();
        meter?.SetFixture();
        var desktop = SystemInformation.VirtualScreen;
        Location = new(desktop.Left - Width - 64, desktop.Top - Height - 64);
        Show();
        PerformLayout();
        Update();
        using var bitmap = new Bitmap(ClientSize.Width, ClientSize.Height);
        DrawToBitmap(bitmap, ClientRectangle);
        bitmap.Save(filename, System.Drawing.Imaging.ImageFormat.Png);
        Hide();
    }

    protected override void WndProc(ref Message message)
    {
        if (message.Msg == 15) PaintCount++;
        base.WndProc(ref message);
    }

    protected override void Dispose(bool disposingResources)
    {
        if (disposingResources && !IsDisposed)
        {
            disposing = true;
            var trayIcon = tray?.Icon;
            tray?.Dispose();
            trayIcon?.Dispose();
            trayMenu?.Dispose();
            mediaControls?.Dispose();
            meterTimer.Dispose();
            introTimer.Dispose();
            saveTimer.Dispose();
            tools.Dispose();
            player.Dispose();
            lifetime.Dispose();
        }
        base.Dispose(disposingResources);
    }

    private Control Option(string label, string[] entries, string[] values, string selected, Action<string> change)
    {
        var row = Row(0, 165);
        row.Controls.Add(Caption(labels[label], 10, colors.Text), 0, 0);
        var choice = Choice(entries.Select(entry => labels[entry]).ToArray());
        choice.AccessibleName = labels[label];
        choice.SelectedIndex = Math.Max(0, Array.IndexOf(values, selected));
        choice.SelectedIndexChanged += (_, _) => change(values[choice.SelectedIndex]);
        row.Controls.Add(choice, 1, 0);
        return row;
    }

    private ComboBox Choice(string[] entries)
    {
        var choice = new ComboBox { Dock = DockStyle.Fill, DropDownStyle = ComboBoxStyle.DropDownList,
            DrawMode = DrawMode.OwnerDrawFixed, FlatStyle = FlatStyle.Flat, ForeColor = colors.Text,
            BackColor = colors.Surface, Margin = new Padding(0, 4, 0, 4) };
        choice.Items.AddRange(entries);
        choice.DrawItem += (_, e) =>
        {
            var selected = (e.State & DrawItemState.Selected) != 0 && (e.State & DrawItemState.ComboBoxEdit) == 0;
            using var background = new SolidBrush(selected ? colors.AccentSurface : colors.Surface);
            e.Graphics.FillRectangle(background, e.Bounds);
            if (e.Index < 0) return;
            TextRenderer.DrawText(e.Graphics, entries[e.Index], choice.Font, e.Bounds, selected ? colors.Accent : colors.Text,
                TextFormatFlags.VerticalCenter | TextFormatFlags.SingleLine | TextFormatFlags.EndEllipsis);
        };
        return choice;
    }

    private CheckBox Check(string label, bool value, Action<bool> onChange)
    {
        var box = new CheckBox { Text = labels[label], Checked = value, Dock = DockStyle.Fill, ForeColor = colors.Text,
            BackColor = colors.Window, FlatStyle = FlatStyle.Flat, Margin = Padding.Empty };
        box.FlatAppearance.BorderColor = colors.Muted;
        box.FlatAppearance.CheckedBackColor = colors.AccentSurface;
        box.CheckedChanged += (_, _) => onChange(box.Checked);
        return box;
    }

    private TrackBar Slider(int value, int min, int max, string name, Action<int> onChange)
    {
        var bar = new TrackBar { Minimum = min, Maximum = max, Value = value, TickFrequency = 10,
            Dock = DockStyle.Fill, BackColor = colors.Window, AutoSize = false, AccessibleName = name };
        bar.ValueChanged += (_, _) => { onChange(bar.Value); tools.SetToolTip(bar, $"{name} · {bar.Value}"); };
        tools.SetToolTip(bar, $"{name} · {bar.Value}");
        return bar;
    }

    private Button Button(string text, string label, Action action, bool primary = false, bool bold = false)
    {
        var symbol = text.Length == 1;
        var button = new Button { Text = text, FlatStyle = FlatStyle.Flat, Dock = DockStyle.Fill,
            Font = new(symbol ? "Segoe UI Symbol" : Font.Name, symbol ? 14 : 10, bold ? FontStyle.Bold : FontStyle.Regular),
            Margin = new Padding(2, 3, 2, 3), UseVisualStyleBackColor = false, Cursor = Cursors.Hand };
        ApplyButton(button, primary);
        NameButton(button, label);
        button.Click += (_, _) => action();
        return button;
    }

    private void NameButton(Button button, string name)
    {
        if (button.AccessibleName == name) return;
        button.AccessibleName = name;
        tools.SetToolTip(button, name);
    }

    private void ApplyButton(Button button, bool primary)
    {
        button.BackColor = primary ? colors.AccentSurface : colors.Surface;
        button.ForeColor = primary ? colors.Accent : colors.Text;
        button.FlatAppearance.BorderColor = primary ? colors.Accent : colors.Border;
        button.FlatAppearance.MouseOverBackColor = primary ? colors.AccentSurface : colors.Raised;
        button.FlatAppearance.MouseDownBackColor = colors.AccentSurface;
    }

    private Label Caption(string text, float size, Color color, bool bold = false, bool center = false, int padding = 0) =>
        new() { Text = text, Dock = DockStyle.Fill, ForeColor = color, AutoEllipsis = true,
            TextAlign = center ? ContentAlignment.MiddleCenter : ContentAlignment.MiddleLeft,
            Font = new(Font.FontFamily, size, bold ? FontStyle.Bold : FontStyle.Regular),
            Margin = Padding.Empty, Padding = new Padding(padding, 0, 0, 0), UseMnemonic = false };

    private static TableLayoutPanel Column(params int[] heights)
    {
        var table = new TableLayoutPanel { Dock = DockStyle.Fill, Margin = Padding.Empty, ColumnCount = 1, RowCount = heights.Length };
        table.ColumnStyles.Add(new ColumnStyle(SizeType.Percent, 100));
        foreach (var height in heights) table.RowStyles.Add(height == 0 ? new(SizeType.Percent, 100) : new(SizeType.Absolute, height));
        return table;
    }

    private static TableLayoutPanel Row(params int[] widths)
    {
        var table = new TableLayoutPanel { Dock = DockStyle.Fill, Margin = Padding.Empty, ColumnCount = widths.Length, RowCount = 1 };
        table.RowStyles.Add(new RowStyle(SizeType.Percent, 100));
        foreach (var width in widths) table.ColumnStyles.Add(width == 0 ? new(SizeType.Percent, 100) : new(SizeType.Absolute, width));
        return table;
    }

    private static IEnumerable<Control> AllControls(Control root) =>
        root.Controls.Cast<Control>().SelectMany(child => AllControls(child).Prepend(child));

    [System.Runtime.InteropServices.DllImport("kernel32.dll", EntryPoint = "GetModuleHandleW", CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
    private static extern nint GetModuleHandle(string? name);
    [System.Runtime.InteropServices.DllImport("comctl32.dll")]
    private static extern int LoadIconMetric(nint module, nint name, int metric, out nint icon);
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    private static extern bool DestroyIcon(nint icon);
}

internal sealed class SearchField(string hint, Color color) : TextBox
{
    protected override void OnTextChanged(EventArgs e)
    {
        base.OnTextChanged(e);
        Invalidate();
    }

    protected override void WndProc(ref Message message)
    {
        base.WndProc(ref message);
        if (message.Msg is not (0xF or 0x318) || TextLength != 0 || Focused || !IsHandleCreated) return;
        using var graphics = message.Msg == 0x318 && message.WParam != 0 ? Graphics.FromHdc(message.WParam) : CreateGraphics();
        TextRenderer.DrawText(graphics, hint, Font, new Rectangle(0, 0, Width, Height), color,
            TextFormatFlags.NoPadding | TextFormatFlags.SingleLine | TextFormatFlags.VerticalCenter);
    }
}

internal sealed record Palette(Color Window, Color Surface, Color Raised, Color Border, Color Text, Color Muted, Color Accent, Color AccentSurface)
{
    public static readonly Palette Graphite = new(H(0x141b22), H(0x1e2933), H(0x283643), H(0x344451), H(0xeef5f8), H(0xa8bac7), H(0x44e1bf), H(0x223b3b));
    public static readonly Palette Amber = new(H(0x211b16), H(0x31261c), H(0x443525), H(0x604a34), H(0xfff3df), H(0xd5bfa5), H(0xffbb61), H(0x51391e));
    private static Color H(int rgb) => Color.FromArgb(unchecked((int)0xff000000) | rgb);
}

internal sealed class StationList : ListBox
{
    private readonly Palette colors;
    private readonly string language;
    private readonly Font titleFont = new("Segoe UI", 10f, FontStyle.Bold);
    private readonly Font detailFont = new("Segoe UI", 8.4f);
    [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
    internal string? PlayingId { get; set; }
    [DesignerSerializationVisibility(DesignerSerializationVisibility.Hidden)]
    internal string EmptyMessage { get; set; } = "";

    public StationList(Palette colors, string language)
    {
        this.colors = colors;
        this.language = language;
        DrawMode = DrawMode.OwnerDrawFixed;
        DisplayMember = nameof(Station.Name);
        BorderStyle = BorderStyle.None;
        IntegralHeight = false;
        BackColor = colors.Window;
        ForeColor = colors.Text;
        ItemHeight = 40;
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        ItemHeight = (int)Math.Round(40 * DeviceDpi / 96.0);
    }

    protected override void OnDrawItem(DrawItemEventArgs e)
    {
        if (e.Index < 0 || e.Index >= Items.Count) return;
        var station = (Station)Items[e.Index];
        var b = e.Bounds;
        var selected = (e.State & DrawItemState.Selected) != 0;
        var playing = station.Id == PlayingId;
        var rowColor = playing ? colors.AccentSurface : selected ? colors.Raised : e.Index % 2 == 0 ? colors.Surface : colors.Window;
        using var background = new SolidBrush(rowColor);
        e.Graphics.FillRectangle(background, b);
        var color = playing ? colors.Accent : colors.Text;
        var flags = TextFormatFlags.Left | TextFormatFlags.VerticalCenter | TextFormatFlags.SingleLine |
                    TextFormatFlags.EndEllipsis | TextFormatFlags.NoPrefix;
        var first = new Rectangle(b.X + 32, b.Y + 2, Math.Max(0, b.Width - 43), b.Height / 2 + 2);
        var second = new Rectangle(first.X, b.Y + b.Height / 2, first.Width, b.Height / 2 - 2);
        TextRenderer.DrawText(e.Graphics, station.Name, titleFont, first, color, flags);
        TextRenderer.DrawText(e.Graphics, station.Detail(language), detailFont, second, colors.Muted, flags);
        var dot = new Rectangle(b.X + 12, b.Y + (b.Height - 7) / 2, 7, 7);
        if (playing)
        {
            using var brush = new SolidBrush(colors.Accent);
            e.Graphics.FillEllipse(brush, dot);
        }
        else
        {
            using var pen = new Pen(colors.Muted);
            e.Graphics.DrawEllipse(pen, dot);
        }
        if (selected && Focused) ControlPaint.DrawFocusRectangle(e.Graphics, b, color, rowColor);
    }

    protected override void WndProc(ref Message message)
    {
        base.WndProc(ref message);
        if (message.Msg is not (0xF or 0x318) || Items.Count != 0 || string.IsNullOrEmpty(EmptyMessage) || !IsHandleCreated) return;
        using var graphics = message.Msg == 0x318 && message.WParam != 0 ? Graphics.FromHdc(message.WParam) : CreateGraphics();
        TextRenderer.DrawText(graphics, EmptyMessage, Font, ClientRectangle, colors.Muted,
            TextFormatFlags.HorizontalCenter | TextFormatFlags.VerticalCenter | TextFormatFlags.WordBreak);
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing) { titleFont.Dispose(); detailFont.Dispose(); }
        base.Dispose(disposing);
    }
}
