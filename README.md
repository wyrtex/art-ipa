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

1. **`rfd` (файловые диалоги)** — на iOS не было бэкенда. В `shims/rfd` добавлен `src/backend/ios.rs`, который **никогда не блокирует UI-поток** (вложенный run loop роняет winit на iOS):
   - открыть — выбор «Фото и видео» (медиатека, `PHPickerViewController`; файлы копируются в «Документы/Imported») или «Файлы» (`UIDocumentPickerViewController`). Выбранный файл приходит в приложение на следующем кадре как «перетащенный на окно» — так его открывают/импортируют все Craft-приложения (SoundCraft импортирует аудио/MIDI, редакторы открывают документы);
   - сохранить — в папку «Документы» приложения (видна в «Файлы» → «На iPhone/iPad» → *приложение*);
   - сообщения — `UIAlertController`.
2. **`arboard` (буфер обмена)** — на iOS подхватывал Linux/Wayland-бэкенд и не компилировался. В `shims/arboard` — бэкенд на `UIPasteboard` (текст, HTML, PNG).
3. **`egui-winit` (окно ↔ egui)** — в `shims/egui-winit`: интерфейс не заходит под вырез/Dynamic Island и скруглённые углы (safe area берётся прямо из view окна), а на экранах уже 1280 pt рисуется мельче, чтобы десктопный интерфейс помещался; доставка выбранных в пикере файлов.
4. **Ветки «Linux-десктоп».** Приложения пишут `cfg(all(unix, not(target_os = "macos")))`, имея в виду Linux, но под это условие попадает и iOS — и тянет за собой X11/Wayland. `prepare_app.py` переписывает такие условия, чтобы они исключали iOS.
5. **Пути.** Десктопный код на iOS идёт по «Linux»-ветке. `shims/craft-ios` при запуске направляет `XDG_*` в `Library/Application Support` и `Library/Caches`, а `$HOME` и рабочую папку делает `Documents`, так что поля «путь» по умолчанию указывают на видимую в «Файлах» папку.
6. **OpenGL (`glow`).** LightCraft и PdfCraft используют OpenGL как запасной рендерер, а на iOS его нет. Патчи в `patches/lightcraft` и `patches/pdfcraft` оставляют на iOS только Metal (через wgpu).

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
- На iPhone только альбомная ориентация: в портретной десктопный интерфейс не помещается. На iPad — любая.
- Интерфейс держится внутри безопасной зоны (вырез, скругления), а на экранах уже 1280 pt рисуется мельче, чтобы поместиться целиком.
- Вопросы «Да/Нет» через системный диалог, вызванные из UI-потока, не могут ждать ответа и получают «осторожный» ответ (Нет/Отмена). Обычно приложения спрашивают в своём собственном интерфейсе, так что это редкость.
- Файл из системного пикера приходит в приложение как «перетащенный на окно». Поэтому, например, «Вставить картинку» в GridCraft откроет её как документ, а не вставит в лист.
- Тап работает как клик. Экранная клавиатура появляется, когда фокус в текстовом поле, и вводит текст и Backspace. Стрелки и сочетания ⌘ с аппаратной клавиатуры `winit 0.30` на iOS пока не передаёт.
- Нет многооконности. Нет печати (`craft-print` на iOS ничего не находит). Нет «Открыть с помощью» из других приложений.
- На этой машине сборка проверялась только через `cargo check --target aarch64-apple-ios`: линковка требует iOS SDK, а он есть только на macOS. Окончательная проверка — это CI на macOS-раннере и запуск на реальном устройстве.

## Обновление на новые версии upstream

Поменяйте `rev` в `apps.json`. Если upstream изменился так, что iOS-сборка ломается, положите исправление в `patches/<app>/NN-описание.patch` (формат `git diff` от корня репозитория приложения).

## Лицензии

Приложения ArtCraft, `rfd` и `arboard` распространяются под MIT / Apache-2.0. Шимы сохраняют исходные лицензии, а iOS-бэкенды в них отмечены комментарием `added by art-ipa`.
