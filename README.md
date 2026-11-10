# Unofficial Telegram Desktop Rust

Неофициальный клиент Telegram для Windows, macOS и Linux, переписанный на Rust
с упором на максимальную производительность: GPU-рендеринг всего интерфейса,
разбор протокола без лишних копирований, многопоточная обработка данных и мгновенный
старт из локального кэша.

> **Статус:** этап 0 — каркас проекта. Клиентом пока пользоваться нельзя.
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

## Структура

```
crates/tl-parser/   парсер и diff TL-схем
xtask/              автоматизация (отслеживание upstream)
schema/             TL-схемы, скопированные из tdesktop (не править руками)
upstream/           lock-файл, карта соответствия папок, отчёты о синхронизации
docs/               документация
```

## Лицензия

[GPL-3.0-or-later](LICENSE) — проект является производной работой от
[Telegram Desktop](https://github.com/telegramdesktop/tdesktop) (GPLv3).
