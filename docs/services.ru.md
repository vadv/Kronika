# Сервисы systemd

[English version](services.md) · [Установка](../INSTALL.ru.md)

Эти units используют binaries в `/usr/local/bin`, хранилище владельца root и
HTTP listener на loopback. У каждого корня хранения один collector writer и
один web index owner. Перед запуском units остановите foreground processes.

## Environment files

Создайте и отредактируйте файлы:

```sh
sudo install -d -m 0700 /etc/kronika /var/lib/kronika
sudo touch /etc/kronika/collector.env /etc/kronika/web.env
sudo chmod 0600 /etc/kronika/collector.env /etc/kronika/web.env
sudoedit /etc/kronika/collector.env /etc/kronika/web.env
```

`/etc/kronika/collector.env`:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_RETENTION=2147483648
```

`/etc/kronika/web.env` — замените пароль:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_WEB_LISTEN=127.0.0.1:8080
KRONIKA_WEB_SOURCES=1
KRONIKA_WEB_USER=kronika
KRONIKA_WEB_PASSWORD=replace-with-a-random-password
```

Systemd разбирает эти строки как присваивания environment variables. Значения
с пробелами целиком заключаются в кавычки; shell substitutions и `export`
не вычисляются.

После [создания роли PostgreSQL](../INSTALL.ru.md#5-postgresql) добавьте в `collector.env`:

```ini
KRONIKA_PG_DSNS="host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres"
```

Для локального PostgreSQL с теми же ресурсами VM/контейнера не задавайте
`KRONIKA_POSTGRES_EFFECTIVE_CPUS`. Для удалённого PostgreSQL или другой cgroup
задайте ёмкость CPU целевого сервера положительным целым числом, например
`KRONIKA_POSTGRES_EFFECTIVE_CPUS=4`. [Расчёт ёмкости](metrics-time.ru.md#health).
`KRONIKA_WEB_SOURCES=3` отмечает OS и PostgreSQL
как настроенные в каталоге web. Все параметры:
[collector](../bins/kronika-collector/README.ru.md) и
[web](../bins/kronika-web/README.ru.md).

## Units

Создайте эти файлы через `sudoedit`:

`/etc/systemd/system/kronika-collector.service`:

```ini
[Unit]
Description=Kronika machine history collector
After=network.target

[Service]
Type=simple
User=root
UMask=0077
EnvironmentFile=/etc/kronika/collector.env
ExecStart=/usr/local/bin/kronika-collector
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

`/etc/systemd/system/kronika-web.service`:

```ini
[Unit]
Description=Kronika history web and MCP
After=network.target

[Service]
Type=simple
User=root
UMask=0077
EnvironmentFile=/etc/kronika/web.env
ExecStart=/usr/local/bin/kronika-web
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Оба сервиса используют `UMask=0077`. Collector читает защищённые данные процессов
и логов; web создаёт indexes и ownership locks в том же корне хранения. Web
читает запись и после остановки collector. Export создаёт временные ZMS/HTML
в `TMPDIR` или системном временном каталоге.

## Запуск

```sh
sudo systemd-analyze verify /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now kronika-collector.service kronika-web.service
sudo systemctl status kronika-collector.service kronika-web.service
sudo journalctl -u kronika-collector -u kronika-web --since '5 minutes ago'
```

Откройте <http://127.0.0.1:8080/>. Тот же listener обслуживает `/mcp`.
[SSH forwarding](../INSTALL.ru.md#4-запуск-web) даёт доступ с другой машины.

## Операции

| Операция | Команда |
| --- | --- |
| Лог collector | `sudo journalctl -u kronika-collector -f` |
| Лог web | `sudo journalctl -u kronika-web -f` |
| Немедленный сбор; публикация при добавлении данных в непустой сегмент | `sudo systemctl kill --kill-whom=main --signal=SIGUSR2 kronika-collector` |
| Применение environment changes | `sudo systemctl restart kronika-collector kronika-web` |
| Остановка сбора с сохранением файлов | `sudo systemctl stop kronika-collector` |
| Отключение автозапуска и остановка web | `sudo systemctl disable --now kronika-web` |
| Запуск web | `sudo systemctl start kronika-web` |
| Размер хранения | `sudo du -sh /var/lib/kronika` |

## Замена binaries

Проверьте и распакуйте следующий архив по [инструкции установки](../INSTALL.ru.md#1-скачивание-и-распаковка).
В распакованном каталоге:

```sh
for binary in kronika-collector kronika-web kronika-dump kronika-report; do
  "./$binary" --version
done
sudo systemctl stop kronika-collector kronika-web
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
sudo systemctl start kronika-collector kronika-web
```

Конфигурация остаётся в `/etc/kronika`; записи остаются в `/var/lib/kronika`.

## Удаление сервисов

```sh
sudo systemctl disable --now kronika-collector kronika-web
sudo rm /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
```

Команды удаляют два units. Binaries, конфигурация и записи остаются.
