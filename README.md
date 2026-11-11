# Unofficial Telegram Desktop Rust

Неофициальный клиент Telegram для Windows, macOS и Linux, переписанный на Rust
с упором на максимальную производительность: GPU-рендеринг всего интерфейса,
разбор протокола без лишних копирований, многопоточная обработка данных и мгновенный
старт из локального кэша.

> **Статус:** этап 1 (протокол) завершён. Графического интерфейса пока нет; есть
> консольный клиент `tdr` для проверки сетевого стека.
> Это неофициальный проект, он не связан с Telegram FZ-LLC и работает через
> [Telegram API](https://core.telegram.org/api).

## Документы

- [Архитектура и стек](docs/ARCHITECTURE.md) — почему Rust, откуда возьмётся скорость, выбор UI, структура crates, этапы.
- [Журнал оптимизаций](docs/OPTIMIZATIONS.md) — что в оригинале можно сделать быстрее и что уже сделано.
- [Синхронизация с оригиналом](docs/UPSTREAM.md) — как мы автоматически отслеживаем релизы
  [tdesktop](https://github.com/telegramdesktop/tdesktop).

## Синхронизировано с

tdesktop **v7.4.1**, TL-слой **233** — актуальное значение в
[`upstream/upstream.lock.toml`](upstream/upstream.lock.toml).

## Сборка

Нужен Rust stable (ставится автоматически через `rust-toolchain.toml`).
Для подключения к Telegram скопируйте `.env.example` в `.env` и впишите свои
`api_id`/`api_hash` с [my.telegram.org](https://my.telegram.org).

```bash
cargo test --workspace
cargo xtask upstream check
```

Консольный клиент:

```bash
cargo run --release -p tdr-cli -- ping         # подключение и замеры
cargo run --release -p tdr-cli -- login        # вход по QR-коду
cargo run --release -p tdr-cli -- login phone  # вход по номеру телефона
cargo run --release -p tdr-cli -- me           # аккаунт и время запуска
cargo run --release -p tdr-cli -- dialogs 30   # список чатов
cargo run --release -p tdr-cli -- avatar       # скачать свою аватарку
```

Самотесты сети на реальном аккаунте: `selftest reconnect` (обрыв соединения),
`selftest unknown-key` (сервер «забыл» ключ), `selftest dc 4` (другой DC).
Временные ключи сохраняются между запусками (быстрый старт); `TDR_KEEP_TEMP_KEYS=0`
возвращает поведение tdesktop.

Если PowerShell открывается не в папке проекта (а из-за `[` `]` в имени перейти в неё
командой `cd` не получается), укажите путь к проекту явно:

```bash
cargo run --release --manifest-path "C:\путь\к\проекту\Cargo.toml" -p tdr-cli -- me
```

Сессия хранится в `data/session.bin` (не попадает в git). Это полный доступ к аккаунту:
файл никому не передавайте.

### Профили сборки

| Профиль | Команда | Для чего | Время (4 ядра) |
|---|---|---|---|
| `dev` | `cargo build` | отладка, тесты | правка файла ≈ 3–5 с |
| `release` | `cargo build --release` | ежедневная работа и замеры скорости | правка файла ≈ 3–20 с |
| `dist` | `cargo build --profile dist` | то, что получают пользователи: полный LTO, меньший размер (1,9 МБ против 3,6 МБ у CLI) | ≈ 4 мин с нуля |

Первая сборка после смены профиля долгая, потому что пересобираются все зависимости.
Крейт `tdr-tl` (≈ 5 МБ сгенерированного кода) пересобирается только при обновлении
TL-схемы. На Windows используется линкер `rust-lld` (см. `.cargo/config.toml`).

## Структура

```
crates/tl-parser/   парсер и diff TL-схем
crates/tl-gen/      генератор Rust-кода из TL-схемы
crates/tl/          типы Telegram API (генерируются при сборке)
crates/crypto/      криптография MTProto 2.0
crates/mtproto/     транспорт, ключи, сессии, переподключение
crates/api/         ядро клиента: несколько DC, PFS, миграции, сохранение сессии
crates/cli/         консольный клиент `tdr`
xtask/              автоматизация (отслеживание upstream)
schema/             TL-схемы, скопированные из tdesktop (не править руками)
upstream/           lock-файл, карта соответствия папок, отчёты о синхронизации
docs/               документация
```

## Лицензия

[GPL-3.0-or-later](LICENSE) — проект является производной работой от
[Telegram Desktop](https://github.com/telegramdesktop/tdesktop) (GPLv3).
