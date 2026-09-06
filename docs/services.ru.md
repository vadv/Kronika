# Постоянный сбор через systemd

[English version](services.md) · [Установка](../INSTALL.ru.md)

Используйте эти units после успешного запуска в терминале. Они запускают
установленные программы напрямую, с закрытым хранилищем и web на loopback.
Для одного корня хранения работают один collector и один web. Перед запуском
сервисов остановите соответствующие процессы в терминалах.

## Закрытая конфигурация

Создайте каталоги и файлы, затем отредактируйте их:

```sh
sudo install -d -m 0700 /etc/kronika /var/lib/kronika
sudo touch /etc/kronika/collector.env /etc/kronika/web.env
sudo chmod 0600 /etc/kronika/collector.env /etc/kronika/web.env
sudoedit /etc/kronika/collector.env /etc/kronika/web.env
```

`/etc/kronika/collector.env` для Linux без БД:

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

Это файлы окружения systemd, не shell-скрипты: `export` и подстановки оболочки
не нужны. Значение с пробелами заключайте в кавычки целиком. После создания
роли наблюдения добавьте PostgreSQL в `collector.env`:

```ini
KRONIKA_PG_DSNS="host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres"
```

Задавайте `KRONIKA_POSTGRES_EFFECTIVE_CPUS` только равным известной
положительной целочисленной ёмкости CPU этого сервера PostgreSQL. Измените sources в web на
`3`, затем перезапустите оба сервиса. Остальные переменные перечислены в
руководствах [коллектора](../bins/kronika-collector/README.ru.md) и
[web](../bins/kronika-web/README.ru.md).

## Units

Создайте `/etc/systemd/system/kronika-collector.service` через `sudoedit`:

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

Создайте `/etc/systemd/system/kronika-web.service`:

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

Коллектору нужен привилегированный доступ к процессам и журналам. В этой
простой настройке web работает под той же учётной записью: файлы коллектора
закрыты, а web пишет индексы рядом с ними. Другим пользователям доступ к
каталогу не выдаётся. Web читает запись и после остановки коллектора, поэтому
его unit не зависит от работающего collector. Экспорт использует временное
место на диске; в ограниченном окружении можно задать доступный на запись
`TMPDIR`, как описано в руководстве web.

Проверьте и запустите:

```sh
sudo systemd-analyze verify /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now kronika-collector.service kronika-web.service
sudo systemctl status kronika-collector.service kronika-web.service
sudo journalctl -u kronika-collector -u kronika-web --since '5 minutes ago'
```

Откройте <http://127.0.0.1:8080/>. Для другой машины используйте SSH forwarding
из [инструкции установки](../INSTALL.ru.md). `/mcp` работает на том же адресе
с той же аутентификацией; третий unit не нужен.

## Повседневные операции

| Задача | Команда / действие |
| --- | --- |
| Смотреть ошибки коллектора и стоимость записи | `sudo journalctl -u kronika-collector -f` |
| Смотреть запуск и ошибки запросов web | `sudo journalctl -u kronika-web -f` |
| Собрать окно прямо сейчас | `sudo systemctl kill --kill-whom=main --signal=SIGUSR2 kronika-collector` |
| Применить изменённые файлы окружения | `sudo systemctl restart kronika-collector kronika-web` |
| Остановить сбор, сохранив файлы | `sudo systemctl stop kronika-collector` |
| Запускать web только при необходимости | `sudo systemctl disable --now kronika-web`; позднее `sudo systemctl start kronika-web` |
| Посмотреть занятое место | `sudo du -sh /var/lib/kronika` |

## Заменить программы или удалить сервисы

Сначала проверьте новый архив, его `BUILDINFO` и вывод `--version` всех четырёх
программ. Остановите оба сервиса, сохраните прежние бинарники и архив,
установите новые программы по [инструкции](../INSTALL.ru.md) и запустите
сервисы. Конфигурация и запись остаются в прежних каталогах. Не запускайте
два коллектора над одним каталогом. Совместимость хранения определяется
контрактом формата релиза; наличие старого бинарника само по себе не
гарантирует возможность отката.

Для удаления сервисов отключите автозапуск и остановите их, удалите только
два установленных вами unit-файла и выполните `systemctl daemon-reload`.
Удаляйте четыре программы из `/usr/local/bin`, только если они больше не нужны.
Конфигурация и записи хранятся отдельно: удаление программ не требует
удаления истории.

## Если первый запуск не удался

| Результат | Что проверить |
| --- | --- |
| `Exec format error` | Совпадение `uname -m` и target архива, контрольную сумму. |
| `Permission denied` при запуске программы | Право исполнения и разрешённый запуск на файловой системе. |
| Collector отвергает хранилище | Точный `KRONIKA_STORAGE_DIR`, настоящий каталог, права и уже работающий writer. |
| Web отвергает конфигурацию | Четыре обязательных значения: storage, sources, user, password. |
| Web не создаёт индекс | Учётной записи нужен доступ на запись в тот же закрытый каталог. |
| Нет строк PostgreSQL | Журнал коллектора, первый DSN, `pg_monitor`, `CONNECT` к базам и локальные права расширений. |
| Нет числа PostgreSQL health | Положительный `KRONIKA_POSTGRES_EFFECTIVE_CPUS` и записанные данные Activity. |
| Вместо ресурса прочерк | Справку поля и ошибку чтения в журнале коллектора; отсутствующие поля остаются null. |

`--version` никогда не открывает хранилище и работает даже при отсутствующей
или некорректной конфигурации обычного запуска.
