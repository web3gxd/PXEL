# ADR-010: formula intent и durable bound AST

Дата: 2026-09-29. Статус: принято для реализации среза фазы 2 по ADR-008.

## Контекст

Первый журнал использовал Operation и для Command, и для canonical event.
Для SetCellValue это не меняло смысл, но formula command не может принимать
bound AST клиента. Source и результат server binding имеют разные роли.

## Решение

Command.operation теперь CommandPayload. SetCellFormula содержит address и
ограниченный FormulaSource; canonical Operation содержит address и проверенный
Formula. Receipt/fingerprint относятся к исходному намерению, включая source и
base version, а не к рассчитанному заново AST. Проверка receipt после ACL идет
до исторического binding, поэтому retry давно committed операции не зависит от
доступности старого transformation window.

CellContent отделяет authored Value от Formula; calculated cache в workbook hash
не входит. Старые value-only domain hashes сохраняют прежние bytes/tags; Formula
получает отдельный tag и deterministic semantic hash всех persisted полей.
Domain AST имеет плоские versioned nodes, stable targets/modes, source/canonical,
profile/origin/base; diagnostic spans отсутствуют. Decode валидирует структуру.

Persisted formula event использует envelope schema 2; legacy events остаются
schema 1. Старые записи не переписываются. Wire имеет собственные formula DTOs
в отдельном namespace, без импорта storage schema. Изначально поля аналогичны,
но их code generation и compatibility policy независимы. DocumentCommand
получает отдельное additive formula_command field; ровно один payload допустим.
Canonical formula AST во входящем legacy operation отвергается.

## Binding в первом срезе

В текущем поддержанном наборе операций оси неизменны после CreateWorkbook.
Поэтому для base >= 1 в пределах последних 10000 версий оси совпадают с head.
Binder использует эту проекцию, а Formula.bound_at_version записывает base.
Base 0, future base и превышение окна отвергаются; receipt ранее сохраненной
операции проверяется раньше. Это доказательство применимо только до появления
структурных операций. Их поддержка требует сначала checkpoint+structural tail
и полноценного алгоритма docs/collaboration.md; расширять набор операций,
сохранив предположение неизменности осей, запрещено. Structural history bytes
в этом срезе равны нулю; лимит 16 MiB остается обязательным для фазы 4.

## Проверки и миграция

Нужны старые golden hashes, legacy replay, formula envelope roundtrip/schema
rejection, source fingerprint collision, candidate failure без записи и реальный
DB restart. Существующие документы не мигрируются в другой hash задним числом.
Formula source и AST относятся к document content и не логируются.

Calculation transport, contexts и stateful browser preview проходят отдельные
проверки; наличие durable formula не означает завершение server calculation.
