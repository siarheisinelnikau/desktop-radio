namespace DesktopRadio;

internal sealed class Labels(string language)
{
    public string this[string english] => language switch
    {
        "ru" when Translations.TryGetValue(english, out var translation) => translation.Russian,
        "kk" when Translations.TryGetValue(english, out var translation) => translation.Kazakh,
        _ => english
    };

    public string Group(string group) => this[group switch
    {
        "BELARUS" => "Belarus",
        "RUSSIA" => "Russia",
        "KAZAKHSTAN" => "Kazakhstan",
        "INTERNATIONAL" => "World",
        _ => "Favorites"
    }];

    private static readonly Dictionary<string, (string Russian, string Kazakh)> Translations = new()
    {
        ["Belarus"] = ("Беларусь", "Беларусь"),
        ["Russia"] = ("Россия", "Ресей"),
        ["Kazakhstan"] = ("Казахстан", "Қазақстан"),
        ["World"] = ("Мир", "Әлем"),
        ["Favorites"] = ("Избранное", "Таңдаулылар"),
        ["Search by name or frequency"] = ("Поиск по названию или частоте", "Атауы немесе жиілігі бойынша іздеу"),
        ["No matching stations"] = ("Станции не найдены", "Станциялар табылмады"),
        ["No favorites yet"] = ("Здесь будут любимые станции", "Таңдаулы станциялар әзірге жоқ"),
        ["No visible stations"] = ("Нет видимых станций", "Көрінетін станциялар жоқ"),
        ["Play"] = ("Включить", "Ойнату"),
        ["Stop"] = ("Остановить", "Тоқтату"),
        ["Previous station"] = ("Предыдущая станция", "Алдыңғы станция"),
        ["Next station"] = ("Следующая станция", "Келесі станция"),
        ["Add to favorites"] = ("В избранное", "Таңдаулыларға қосу"),
        ["Remove from favorites"] = ("Убрать из избранного", "Таңдаулылардан алып тастау"),
        ["Volume"] = ("Громкость", "Дыбыс деңгейі"),
        ["On air"] = ("В эфире", "Эфирде"),
        ["Connecting…"] = ("Подключение…", "Қосылуда…"),
        ["Station unavailable. Try again."] = ("Станция недоступна. Попробуйте снова.", "Станция қолжетімсіз. Қайталап көріңіз."),
        ["Audio output failed. Check your sound device."] = ("Не удалось вывести звук. Проверьте аудиоустройство.", "Дыбыс шықпады. Аудио құрылғыны тексеріңіз."),
        ["Install VLC 3 (64-bit) to listen."] = ("Для прослушивания установите VLC 3 (64 бит).", "Тыңдау үшін VLC 3 (64 бит) орнатыңыз."),
        ["Ready to play"] = ("Готово к воспроизведению", "Ойнатуға дайын"),
        ["Choose a station"] = ("Выберите станцию", "Станцияны таңдаңыз"),
        ["Settings"] = ("Настройки", "Баптаулар"),
        ["Equalizer"] = ("Эквалайзер", "Эквалайзер"),
        ["Enable equalizer"] = ("Включить эквалайзер", "Эквалайзерді қосу"),
        ["Reset bands"] = ("Сбросить полосы", "Жолақтарды қалпына келтіру"),
        ["Band"] = ("Полоса", "Жолақ"),
        ["Done"] = ("Готово", "Дайын"),
        ["Radio"] = ("Радио", "Радио"),
        ["Open radio"] = ("Открыть радио", "Радионы ашу"),
        ["Quit"] = ("Выйти", "Шығу"),
        ["Language"] = ("Язык", "Тіл"),
        ["Theme"] = ("Тема", "Тақырып"),
        ["Graphite"] = ("Графит", "Графит"),
        ["Amber"] = ("Янтарь", "Кәріптас"),
        ["Visualization"] = ("Визуализация", "Визуалдау"),
        ["Spectrum"] = ("Спектр", "Спектр"),
        ["Waveform"] = ("Звуковая волна", "Дыбыс толқыны"),
        ["Off"] = ("Выкл.", "Өшіру"),
        ["Smooth visualization"] = ("Плавная визуализация", "Бірқалыпты визуалдау"),
        ["Show intro when opening the panel"] = ("Показывать заставку при открытии панели", "Панель ашылғанда заставканы көрсету"),
        ["Resume last station on startup"] = ("Включать последнюю станцию при запуске", "Іске қосылғанда соңғы станцияны ойнату"),
        ["Hide recently unavailable stations"] = ("Скрывать недавно недоступные станции", "Жақында қолжетімсіз болған станцияларды жасыру"),
        ["Manage stations"] = ("Управление станциями", "Станцияларды басқару"),
        ["Visible groups"] = ("Видимые группы", "Көрінетін топтар"),
        ["Station is visible"] = ("Станция видима", "Станция көрінеді"),
        ["Move up"] = ("Выше", "Жоғары"),
        ["Move down"] = ("Ниже", "Төмен"),
        ["Check for updates"] = ("Проверить обновления", "Жаңартуларды тексеру"),
        ["Check for updates on startup"] = ("Проверять обновления при запуске", "Іске қосылғанда жаңартуларды тексеру"),
        ["Checking for updates…"] = ("Проверка обновлений…", "Жаңартулар тексерілуде…"),
        ["You have the latest version."] = ("Установлена последняя версия.", "Ең соңғы нұсқа орнатылған."),
        ["No published Windows build is accessible."] = ("Доступная сборка для Windows не опубликована.", "Қолжетімді Windows жинағы жарияланбаған."),
        ["Could not check for updates."] = ("Не удалось проверить обновления.", "Жаңартуларды тексеру мүмкін болмады."),
        ["Update available"] = ("Доступно обновление", "Жаңарту қолжетімді"),
        ["Download and install"] = ("Скачать и установить", "Жүктеп алу және орнату"),
        ["Downloading…"] = ("Скачивание…", "Жүктелуде…"),
        ["Installing…"] = ("Установка…", "Орнатылуда…"),
        ["Use the portable build to install updates."] = ("Для обновления нужна портативная сборка.", "Жаңарту үшін тасымалды жинақты пайдаланыңыз."),
        ["Could not install the update."] = ("Не удалось установить обновление.", "Жаңартуды орнату мүмкін болмады."),
        ["Could not save settings."] = ("Не удалось сохранить настройки.", "Баптауларды сақтау мүмкін болмады.")
    };
}
