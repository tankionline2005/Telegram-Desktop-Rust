# Как мы успеваем за оригинальным клиентом

tdesktop выпускает релизы по несколько раз в неделю, а схема API (TL-слой) меняется
почти каждый месяц. Отслеживание автоматизировано, чтобы ни одно изменение не
прошло мимо.

## Что где лежит

| Файл | Назначение |
|---|---|
| `upstream/upstream.lock.toml` | С какой версией tdesktop мы сейчас синхронизированы: тег, коммит, TL-слой, контрольные суммы скопированных файлов |
| `upstream/map.toml` | Какие папки tdesktop соответствуют каким нашим crates. Отчёт группирует изменения по этой карте |
| `schema/api.tl`, `schema/mtproto.tl` | Байт-в-байт копии TL-схем из tdesktop. Руками не править — CI проверит контрольные суммы |
| `upstream/reports/<тег>.md` | Полные отчёты по каждой синхронизации — история того, что менялось в оригинале |
| `.github/workflows/upstream.yml` | Автоматическая проверка каждые 6 часов |

## Как это работает

```
каждые 6 часов (GitHub Actions)
  └─ cargo xtask upstream check
       ├─ есть релиз новее, чем в lock? ── нет → конец
       └─ да → собирает отчёт:
            • changelog всех новых версий
            • diff TL-схемы: новые/изменённые/удалённые конструкторы и методы
            • новые строки локализации (лучшая подсказка о новых фичах)
            • какие наши crates затронуты (по upstream/map.toml)
            • изменения в сабмодулях lib_ui, lib_base, lib_tl…
            • список коммитов
  └─ cargo xtask upstream sync — копирует новые схемы, обновляет lock
  └─ открывает (или обновляет) один PR «Upstream: tdesktop vA → vB»
  └─ (опционально) присылает сообщение в Telegram
```

Если до слияния PR вышло ещё несколько релизов — тот же PR обновится и будет
покрывать все версии сразу.

## Что делать, когда пришёл PR

1. **Схема** — сливаем быстро. Кодогенерация (`tdr-tl-gen`) пересоберёт типы,
   компилятор покажет все места, где изменились поля.
2. **Новые фичи** (changelog + новые строки локализации) — заводим задачи
   с меткой `upstream-feature` и ссылкой на отчёт.
3. **Исправления багов** — смотрим, касаются ли они уже портированных частей
   (таблица «Affected areas»). Если да — задача с меткой `upstream-fix`.
4. Если в отчёте есть строка «Not covered by `upstream/map.toml`» — добавляем
   новую папку в карту.

## Локальный запуск

```bash
cargo xtask upstream check            # есть ли обновления, краткий отчёт в консоль
cargo xtask upstream check --save     # + полный отчёт в upstream/reports/
cargo xtask upstream sync             # подтянуть схемы последнего релиза
cargo xtask upstream verify           # проверить, что схемы не правили руками
```

Локально скрипт берёт токен из `GITHUB_TOKEN`/`GH_TOKEN` или из `gh auth token`
(без токена GitHub ограничивает 60 запросами в час — на одну проверку хватает).

Канал `beta` (`--channel beta` или `channel = "beta"` в lock) включает и
бета-версии tdesktop — так новые фичи видно на 1–2 недели раньше.

## Настройка репозитория (один раз)

1. Settings → Actions → General → Workflow permissions: **Read and write
   permissions** и **Allow GitHub Actions to create and approve pull requests**.
2. Опционально, секреты (Settings → Secrets and variables → Actions):
   - `UPSTREAM_TOKEN` — personal access token (`contents` + `pull-requests`).
     PR, открытые стандартным токеном, не запускают CI; с этим токеном — запускают.
   - `TELEGRAM_BOT_TOKEN` и `TELEGRAM_CHAT_ID` — уведомления о новых релизах в Telegram.
3. Проверить вручную: Actions → Upstream tracker → Run workflow.
