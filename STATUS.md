# Статус работ

Ветка: `hayro-pdf`

## PDF backend

- Удалены зависимость и vendor `mupdf`, добавлен `hayro 0.8.0` (pure Rust).
- Сохранён интерфейс `src/pdf.rs`, используемый `reader.rs` и `flow.rs`.
- Работают рендер PNG/BGRA, тайлы, текстовый слой с координатами, ссылки, оглавление и ручной разбор page labels (`pdf::page_labels`).
- Ссылка `Contents` на неразбираемый поток возвращает ошибку рендера вместо белой страницы.
- Проверено на `C:\Users\kirill\Downloads\Frontend-разработчик.pdf`; PNG первой страницы: `target\hayro-check\Frontend-разработчик-p0.png` (595×841).
- Hayro не поддерживает зашифрованные PDF; текстовые границы символов вычисляются по глифам и являются приближёнными. Reflow группирует строки в общий текстовый блок, сложная структура колонок пока теряется. Тайлы пока вырезаются из полного рендера страницы, а кеш команд hayro не переиспользуется между запросами (TODO по памяти/скорости).

## NSH на Windows

- Установлен официальный CLI NSH `0.2.0` в `%LOCALAPPDATA%\nsh`.
- Добавлено отдельное Rust-приложение `nsh-app`: открыть PDF по пути, листать страницы, прокручивать изображение и текстовый слой.
- Standalone Windows runner собран и запущен через NSH.
- Прямой `AppView` в текущий GPUI Liber не интегрирован: `nsh-platform-gpui 0.2.0` требует `gpui-fast 0.1.9`, тогда как основной проект использует GPUI Kit `0.7.1` и его fork-зависимости. Использован разрешённый fallback `nsh run --target windows`.
- NSH 0.2.0 использует unstable `isolate_lowest_one`; для `nsh-app` закреплён установленный nightly `nightly-2026-09-29`. Основной Liber по-прежнему собирается на stable.

## Запуск

Из корня репозитория:

```powershell
cd nsh-app
nsh run --target windows
```

Вставьте путь к PDF в поле и нажмите **Open PDF**; для навигации используйте **Previous/Next**. Для сборки пакета: `nsh package --target windows` из `nsh-app`.

## Последняя проверка

Команды выполнялись из корня Liber:

```text
rtk cargo build
cargo build: 0 errors, 9 warnings (1 crates)

rtk cargo test
cargo test: 56 passed, 1 ignored (1 suite, 0.04s)
```

NSH CLI сообщил при запуске:

```text
✓ built the Rust crate nsh-app-liber (3.6s)
✓ built the Windows runner of nsh-app-liber (debug) (4.1s)
✓ running Liber (Rust, no JS runtime)
```

NSH headless test:

```text
✓ App.test.tsx › shows PDF path input and page controls (522ms)
✓ 1 test: 1 passed, 0 failed (9.5s)
```
