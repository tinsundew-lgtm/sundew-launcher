# LimeCraft для Linux Mint

## Установка (готовая сборка)

Распакуйте архив и выполните в папке:

```bash
./install.sh
```

Лаунчер появится в меню («Игры») и в `~/.local/bin/limecraft-launcher`. Удалить: `./uninstall.sh`
(игры и настройки в `~/.local/share/LimeCraft` останутся).

Можно и без установки: `./limecraft-launcher`.

Нужно: Linux Mint 21 или новее (или другой дистрибутив с glibc 2.35+) и 64-битный процессор.
Обычно ничего ставить не нужно: GTK и библиотеки окна в Mint уже есть. Если лаунчер не
открывается, поставьте их:

```bash
sudo apt install libgtk-3-0 libxkbcommon0 libgl1
```

## Java

Отдельно ставить Java не нужно: нужную версию лаунчер скачивает сам (Eclipse Temurin с
сайта Adoptium, с проверкой SHA-256) в `~/.local/share/LimeCraft/runtime`. Если у вас уже
есть подходящая Java, он возьмёт её; путь можно задать вручную в «Настройки».

## Где хранятся данные

`~/.local/share/LimeCraft` (сборки, настройки, скины). Другое место: переменная
`LIMECRAFT_HOME=/путь`.

## Сборка из исходников

В папке с исходниками: `./packaging/linux/build-linux.sh` (поставит нужные пакеты через `apt`,
соберёт программу и положит результат в `dist/` в корне проекта). Потом `./dist/install.sh`.
Нужен Rust: <https://rustup.rs>. Папка `dist/` в git не попадает.

## Что отличается от Windows

- Скины по-прежнему видны в игре только с модом CustomSkinLoader.
- Импорт сборок ищет Prism Launcher (в том числе Flatpak), PolyMC, Modrinth App и `~/.minecraft`.
- Системный шрифт выбирается из установленных в системе (Ubuntu, DejaVu, Liberation); по умолчанию
  используется встроенный Manrope.
