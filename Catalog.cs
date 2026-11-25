using System.Globalization;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace DesktopRadio;

internal sealed record Station(string Id, string Name, string Label, string Aliases, string[] Streams,
    string Group, string? CountryCode, string? LabelRu, string? LabelKk, int TrackSourceId)
{
    public string Detail(string language)
    {
        var label = language switch { "ru" => LabelRu, "kk" => LabelKk, _ => Label } ?? Label;
        return string.IsNullOrEmpty(CountryCode) ? label : $"{CountryCode} · {label}";
    }

    public string SearchText { get; } = Catalog.Normalize($"{Name} {Label} {Aliases} {CountryCode} {LabelRu} {LabelKk}");
}

internal sealed class Catalog
{
    public const string Favorites = "FAVORITES";
    public static readonly string[] Groups = ["BELARUS", "RUSSIA", "KAZAKHSTAN", "INTERNATIONAL", Favorites];
    public static readonly JsonSerializerOptions Json = new(JsonSerializerDefaults.Web) { WriteIndented = true };

    public Station[] Stations { get; } = Load();

    public IEnumerable<Station> InGroup(string group, Preferences preferences, bool includeHidden = false)
    {
        var stations = group == Favorites
            ? Stations.Where(station => preferences.Favorites.Contains(station.Id))
            : Stations.Where(station => station.Group == group);
        var order = preferences.Order.GetValueOrDefault(group) ?? [];
        var ranks = order.Select((id, index) => (id, index)).DistinctBy(item => item.id)
            .ToDictionary(item => item.id, item => item.index);
        return stations.Where(station => includeHidden || IsVisible(station, preferences))
            .OrderBy(station => ranks.GetValueOrDefault(station.Id, int.MaxValue));
    }

    public bool IsVisible(Station station, Preferences preferences) =>
        !preferences.HiddenStations.Contains(station.Id) &&
        (!preferences.HideUnavailable || !preferences.Unavailable.TryGetValue(station.Id, out var failure) ||
         DateTimeOffset.UtcNow - failure >= TimeSpan.FromDays(1));

    public IEnumerable<Station> Find(string query, Preferences preferences)
    {
        var words = Normalize(query).Split(' ', StringSplitOptions.RemoveEmptyEntries);
        return Groups.Where(group => group != Favorites && !preferences.HiddenGroups.Contains(group))
            .SelectMany(group => InGroup(group, preferences))
            .Where(station => words.All(word => station.SearchText.Contains(word, StringComparison.Ordinal)));
    }

    public static string Normalize(string value)
    {
        var builder = new StringBuilder(value.Length);
        foreach (var rune in value.ToLowerInvariant().Normalize(NormalizationForm.FormD).EnumerateRunes())
        {
            if (Rune.GetUnicodeCategory(rune) is UnicodeCategory.NonSpacingMark or UnicodeCategory.EnclosingMark
                or UnicodeCategory.SpacingCombiningMark) continue;
            builder.Append(Rune.IsLetterOrDigit(rune) ? rune.ToString() : " ");
        }
        return Regex.Replace(builder.ToString(), @"\s+", " ").Trim();
    }

    private static Station[] Load()
    {
        using var stream = Assembly.GetExecutingAssembly().GetManifestResourceStream("DesktopRadio.Stations.json")!;
        return JsonSerializer.Deserialize<Station[]>(stream, Json) ?? [];
    }
}

internal sealed class Preferences
{
    public string Language { get; set; } = "en";
    public string Theme { get; set; } = "graphite";
    public string Visualization { get; set; } = "spectrum";
    public bool SmoothVisualization { get; set; } = true;
    public bool ShowIntro { get; set; } = true;
    public bool PlayOnStartup { get; set; } = true;
    public bool CheckUpdatesOnStartup { get; set; } = true;
    public bool HideUnavailable { get; set; }
    public string Group { get; set; } = "BELARUS";
    public string? StationId { get; set; }
    public int Volume { get; set; } = 65;
    public bool EqualizerEnabled { get; set; }
    public int[] Equalizer { get; set; } = new int[EqualizerProcessor.Frequencies.Length];
    public HashSet<string> Favorites { get; set; } = [];
    public HashSet<string> HiddenStations { get; set; } = [];
    public HashSet<string> HiddenGroups { get; set; } = [];
    public Dictionary<string, List<string>> Order { get; set; } = [];
    public Dictionary<string, DateTimeOffset> Unavailable { get; set; } = [];
}

internal sealed class PreferencesStore(string directory)
{
    public static string DefaultDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "DesktopRadio");

    private string Pathname => Path.Combine(directory, "settings.json");

    public Preferences Load()
    {
        try
        {
            if (!File.Exists(Pathname)) return new();
            var settings = JsonSerializer.Deserialize<Preferences>(File.ReadAllText(Pathname), Catalog.Json) ?? new();
            if (settings.Equalizer?.Length != EqualizerProcessor.Frequencies.Length)
                settings.Equalizer = new int[EqualizerProcessor.Frequencies.Length];
            return settings;
        }
        catch (JsonException) { return new(); }
        catch (IOException) { return new(); }
    }

    public void Save(Preferences settings)
    {
        Directory.CreateDirectory(directory);
        var pending = Pathname + ".tmp";
        File.WriteAllText(pending, JsonSerializer.Serialize(settings, Catalog.Json));
        File.Move(pending, Pathname, overwrite: true);
    }
}
