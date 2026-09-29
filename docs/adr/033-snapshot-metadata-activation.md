# ADR-033: активация локального snapshot через PostgreSQL metadata

Статус: принят для внутреннего storage среза фазы 5, 2026-09-29.

## Контекст

ADR-031/032 определили проверяемый доменный образ и отдельный бинарный codec.
Объект нельзя считать пригодным для восстановления по факту успешной записи:
запись может оборваться до проверки или metadata COMMIT.

## Решение

`document_snapshots` хранит immutable identity, версию, last axis version,
schema, object key, размер, checksum, hash и engine/profile. Trigger допускает
только `UPLOADING → STORED → VERIFIED → ACTIVE` и переходы в `REJECTED`.
`documents.preferred_snapshot_id` указывает snapshot того же документа;
публикация старого позднего job не понижает preferred version. Несколько
поколений остаются `ACTIVE` для fallback.

Publisher сверяет версию и state hash книги с checksum-проверенным
последним journal event, кодирует образ, записывает immutable объект,
читает его обратно, проверяет bytes/checksum/metadata/domain hash, затем
активирует запись короткой транзакцией с блокировкой строки документа.
Reader видит только `ACTIVE`, повторно проверяет объект и codec и пробует
старшие поколения до первого валидного. Недоступный объект не меняет
источник истины: допускается full journal replay.

Внутренний `recover_document` сначала авторизует чтение, выбирает snapshot
не новее зафиксированного head, затем применяет проверенные journal pages до
этой версии. Gap, checksum и ACL ошибки не пропускаются. При отсутствии
валидного образа тот же алгоритм начинает с пустой книги.

## Границы

API storage crate подключён к actor startup по ADR-034; scheduler добавлен
ADR-035. Исторический endpoint добавлен в ADR-036; уборка незавершённых
объектов — в ADR-037, retention старых ACTIVE поколений — в ADR-038.
Позднее optional S3 adapter прошёл локальный Moto contract (ADR-039);
Garage и российский S3 ещё не проверены.
На одном вызове reader проверяет
не более 128 поколений: при их исчерпании возможен full replay. Временную
I/O-ошибку reader возвращает, а повреждённый или отсутствующий объект
пропускает. Production durability Windows filesystem и S3 не заявляются.

## Проверка controlled interruption

Test-only точки остановки после metadata INSERT, blob PUT, переходов STORED
и VERIFIED, UPDATE внутри activation transaction и её COMMIT проверяют
настоящие PostgreSQL/LocalFilesystemBlobStore adapters. До COMMIT reader
видит только full journal replay, а preferred pointer остаётся пустым;
обновление ACTIVE внутри незавершённой transaction откатывается. После
COMMIT reader видит ACTIVE snapshot и preferred pointer. Это не тест
потери питания, повреждения файловой системы или restart процесса.
