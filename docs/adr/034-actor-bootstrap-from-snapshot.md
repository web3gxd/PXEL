# ADR-034: запуск document actor из проверенного snapshot

Статус: принят для среза фазы 5, 2026-09-29.

## Контекст

ADR-033 реализовал проверяемое восстановление snapshot + journal tail, но
document actor при первом открытии всё ещё начинал с пустой книги. Это
оставляло snapshot вне рабочего пути и заставляло заново применять весь log.

## Решение

`InProcessSessionDirectory` принимает опциональный recovery callback. В
server binary callback использует `PgDatabase::recover_document` с локальным
BlobStore; базовый конструктор остаётся полным journal replay для других
реальных конфигураций и тестов. Каталог сначала проверяет доступ и наличие
действующей сессии, затем запускает recovery вне общего mutex. После recovery
он повторно проверяет каталог: при гонке два открытия могут прочитать
один snapshot, но запустится только один actor. Одновременных recovery не
больше четырёх, timeout — 10 секунд.

Actor получает подготовленную подтверждённую книгу. Он запускает полный
расчёт restored projection, поскольку cache результатов не является частью
канонического snapshot; затем `refresh` дочитывает события, появившиеся
после зафиксированной в начале recovery версии. Каждое новое чтение или
команда повторно проверяет ACL. Ошибка recovery не создаёт actor и не
подтверждает команду.

## Последствия и проверка

Snapshot теперь находится в runtime startup path. PostgreSQL/BlobStore тест
проверяет, что session стартует с v2 snapshot без replay и получает
calculated state этой версии. Удалённый/corrupt snapshot выбирает старое
поколение либо полный log. Scheduler, crash injection, S3 и 10k-tail
benchmark остаются отдельными gates.
