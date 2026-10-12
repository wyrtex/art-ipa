# art-ipa

Сборка приложений **ArtCraft Crafting Apps** ([github.com/storytold](https://github.com/storytold)) под iPhone и iPad. На выходе получаются неподписанные `.ipa`.

| Приложение | Что это | Репозиторий |
|---|---|---|
| PhotoCraft | фоторедактор (аналог Photoshop) | [storytold/photocraft](https://github.com/storytold/photocraft) |
| FilmCraft | нелинейный видеоредактор | [storytold/filmcraft](https://github.com/storytold/filmcraft) |
| LightCraft | неразрушающая обработка RAW (аналог Lightroom) | [storytold/lightcraft](https://github.com/storytold/lightcraft) |
| PdfCraft | просмотр и редактирование PDF | [storytold/pdfcraft](https://github.com/storytold/pdfcraft) |
| WordCraft | текстовый редактор (аналог Word) | [storytold/wordcraft](https://github.com/storytold/wordcraft) |
| VectorCraft | векторная графика / SVG | [storytold/vectorcraft](https://github.com/storytold/vectorcraft) |
| EffectCraft | моушн-графика и композитинг | [storytold/effectcraft](https://github.com/storytold/effectcraft) |
| GridCraft | электронные таблицы (аналог Excel) | [storytold/gridcraft](https://github.com/storytold/gridcraft) |
| CADCraft | САПР / черчение (аналог AutoCAD) | [storytold/cadcraft](https://github.com/storytold/cadcraft) |
| DesignCraft | вёрстка и макетирование | [storytold/designcraft](https://github.com/storytold/designcraft) |
| DeckCraft | презентации (аналог PowerPoint) | [storytold/deckcraft](https://github.com/storytold/deckcraft) |
| SoundCraft | работа со звуком (аналог Pro Tools) | [storytold/soundcraft](https://github.com/storytold/soundcraft) |

Исходники самих приложений здесь не копируются. Каждое из них берётся из upstream на зафиксированном коммите (`apps.json`), а потом к нему применяется небольшой набор iOS-адаптаций.

## Как это работает

Все приложения написаны на чистом Rust поверх `eframe`/`egui` + `wgpu`. `eframe`, `winit` и `wgpu` (Metal) уже умеют работать на iOS. Мешали три вещи, и для каждой есть своё решение:

1. **`rfd` (файловые диалоги)** не имеет iOS-бэкенда, поэтому на iOS сборка просто не компилировалась. В `shims/rfd` лежит копия `rfd 0.17.2` с добавленным `src/backend/ios.rs`:
   - **Открыть**: системный `UIDocumentPickerViewController`. Выбранный файл копируется в песочницу приложения.
   - **Сохранить**: файл записывается в папку «Документы» приложения. Она видна в приложении «Файлы» → «На iPhone/iPad» → *имя приложения*.
   - **Сообщения и вопросы**: `UIAlertController`.
2. **`arboard` (буфер обмена)** на iOS подхватывал Linux/Wayland-бэкенд и не компилировался. В `shims/arboard` добавлен бэкенд на `UIPasteboard`: текст, HTML и картинки PNG.
3. **Пути к настройкам.** Десктопный код на iOS идёт по «Linux»-ветке и пишет в `~/.config`, а в песочнице iOS туда писать нельзя. Крейт `shims/craft-ios` при запуске направляет `XDG_*` в `Library/Application Support` и `Library/Caches`, а рабочей папкой делает `Documents`.

4. **Ветки «Linux-десктоп».** Приложения пишут `cfg(all(unix, not(target_os = "macos")))`, имея в виду Linux, но под это условие попадает и iOS — и тянет за собой X11/Wayland. `prepare_app.py` переписывает такие условия, чтобы они исключали iOS.
5. **OpenGL (`glow`).** LightCraft и PdfCraft используют OpenGL как запасной рендерер, а на iOS его нет. Патчи в `patches/lightcraft` и `patches/pdfcraft` оставляют на iOS только Metal (через wgpu).

Скрипт `scripts/prepare_app.py` подключает шимы через `[patch.crates-io]`, добавляет в `main()` вызов `craft_ios::init()` и применяет патчи из `patches/<app>/*.patch`, если они есть. Затем `scripts/build-ipa.sh` собирает бинарник под `aarch64-apple-ios`, а `scripts/make_bundle.py` собирает `.app` (Info.plist, иконки) и упаковывает его в `.ipa`.

## Где взять .ipa

**GitHub Actions** (вкладка *Actions* → *Build IPAs*). Каждое приложение собирается отдельной задачей на `macos-15`, готовые `.ipa` лежат в артефактах запуска. Чтобы собрать только часть приложений, нажмите *Run workflow* и перечислите их id через пробел. Если запушить тег `v*`, все `.ipa` попадут в GitHub Release.

**Локально на Mac** (нужны Xcode и rustup):

```sh
scripts/build-ipa.sh gridcraft      # → dist/GridCraft-<версия>-ios.ipa
```

## Установка на устройство

`.ipa` не подписаны. Их можно поставить через **AltStore / SideStore**, **Sideloadly** (с бесплатным Apple ID приложение работает 7 дней, потом его нужно переподписать) или **TrollStore** (на поддерживаемых версиях iOS работает без ограничений). Минимальная версия: iOS / iPadOS 15.

## Ограничения (честно)

- Приложения не перерисованы под тач. Это десктопные интерфейсы с меню и панелями, поэтому на **iPad** (особенно с клавиатурой и трекпадом) ими пользоваться реально, а на iPhone очень тесно.
- Тап работает как клик. Экранная клавиатура появляется, когда фокус в текстовом поле, и вводит текст и Backspace. Стрелки и сочетания ⌘ с аппаратной клавиатуры `winit 0.30` на iOS пока не передаёт.
- Нет многооконности. Нет печати (`craft-print` на iOS ничего не находит). Нет «Открыть с помощью» из других приложений.
- Безопасные зоны (вырез, «чёлка») не учитываются: статус-бар скрыт, а в альбомной ориентации на iPhone край интерфейса может уходить под вырез.
- На этой машине сборка проверялась только через `cargo check --target aarch64-apple-ios`: линковка требует iOS SDK, а он есть только на macOS. Окончательная проверка — это CI на macOS-раннере и запуск на реальном устройстве.

## Обновление на новые версии upstream

Поменяйте `rev` в `apps.json`. Если upstream изменился так, что iOS-сборка ломается, положите исправление в `patches/<app>/NN-описание.patch` (формат `git diff` от корня репозитория приложения).

## Лицензии

Приложения ArtCraft, `rfd` и `arboard` распространяются под MIT / Apache-2.0. Шимы сохраняют исходные лицензии, а iOS-бэкенды в них отмечены комментарием `added by art-ipa`.
