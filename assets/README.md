# Ассеты

| Путь | Источник | Лицензия |
|---|---|---|
| `icons/` | Иконка проекта (автор — владелец репозитория); `.ico` и PNG разных размеров получены из `app-icon.png` | как у проекта |
| `theme/colors.palette` | `desktop-app/lib_ui`, `ui/colors.palette` (коммит, закреплённый в tdesktop) | GPLv3 (tdesktop) |
| `theme/night.tdesktop-theme` | tdesktop, `Telegram/Resources/night.tdesktop-theme` | GPLv3 (tdesktop) |
| `fonts/OpenSans-*.ttf` | `desktop-app/lib_ui`, `fonts/` — Open Sans 1.10, © Google | Apache License 2.0 |

Файлы из tdesktop и `lib_ui` — побайтовые копии; их версии и контрольные суммы хранятся в
`upstream/upstream.lock.toml`, обновляет их `cargo xtask upstream sync`.
