# Threat model

Статус: foundation accepted for phase 0; implementation evidence находится в RUNBOOK. Локальный skeleton не является production deployment.

## Активы и границы доверия

Активы: document content/history, tenant isolation, ACL, operation integrity/durability, private snapshots/imports/exports, credentials и availability. Недоверенные источники: HTTP/WS clients, client actor_id/AST, clipboard/formulas/XLSX, third-party object-store responses и reconnect metadata.

Trust boundaries: browser→transport; transport→server AuthContext; command→validated operation; application→PostgreSQL; parent→XLSX worker; snapshot bytes→validated domain state. Внешний auth/provider не является источником document ACL; viewer/editor/owner проверяются сервером.

| Угроза                           | Контроль и этап                                                                                                                                                                                      |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Cross-tenant document access     | Tenant-aware queries и ACL каждой команды/subscription/history/export; phase 1                                                                                                                       |
| Forged actor_id                  | Server principal, mismatch rejection; command boundary phase 1                                                                                                                                       |
| Subscription после revoke        | document access_epoch и process-local payload send gate проверены; distributed fencing и membership revoke открыты                                                                                   |
| Duplicate/stale writer           | Operation receipt, payload fingerprint, CAS head, unique versions; phase 1                                                                                                                           |
| Partial durable state / lost ACK | Candidate preapply, one transaction, uncertain commit resolution, replay; phase 1                                                                                                                    |
| Malformed/huge frame             | WS/HTTP byte caps, schema/decode caps, unknown version rejection; phase 0 handshake, расширяется с commands                                                                                          |
| Slow consumer / queue exhaustion | Bounded queues+bytes, deadlines, disconnect, fair admission; actor/subscriptions phases 1/3                                                                                                          |
| Cross-site WS/session use        | Exact Origin allowlist, no query-string credentials; cookie mode требует CSRF token/SameSite policy                                                                                                  |
| Document content in logs         | Allowlist structured fields, отсутствие Debug payload/body; tests/review с phase 0                                                                                                                   |
| XSS / formula content            | Plain text DOM rendering, CSP, no arbitrary HTML or executable macros; editor phase 1                                                                                                                |
| Filesystem traversal             | Opaque validated BlobKey, private root, no symlink escape; phase 0                                                                                                                                   |
| Snapshot corruption              | Schema/length/checksum/domain hash verification, ACTIVE generations fallback; phase 5                                                                                                                |
| Share-token theft/replay         | 256-bit fragment token, only SHA-256 at rest, scoped 24-hour-max guest session cookie, uniform invalid-link errors; single-process revoke gate implemented, distributed invalidation open            |
| PIN/password guessing            | Argon2id verifier encrypted at rest, two bounded workers, PostgreSQL attempt serialization, 5-failure/15-minute lock and owner disable notification                                                  |
| Guest privilege escalation       | Typed member/guest principals, per-link document scope and capabilities, no guest ACL/export/history routes; real Chromium revoke/edit-control test                                                  |
| Comment disclosure               | Independent encrypted revision journal, stable-cell address, role checks, bounded text/pages and no content in logs                                                                                  |
| XLSX CPU/RAM/code exploitation   | Bounded frames/timeout, parent IR validation и tree termination (Windows Job Object/Unix process group) есть; hard resource caps, filesystem/network sandbox и broad hostile corpus остаются открыты |
| Dependency compromise            | License/advisory/source check, lockfiles, no blind major upgrades; ongoing                                                                                                                           |

## Development authentication

Разрешена только отдельным compile feature плюс runtime opt-in; normal production build ее не содержит. Она предназначена для loopback/local development и не смешивается с production auth middleware. Credentials не встраиваются в исходники/инструкции и не передаются query parameter. Без configured auth защищенные endpoints fail closed.

Фаза 0 handshake сама по себе не дает доступ к документам. Текущий dev-auth
остается loopback-only; production identity для владельцев — отдельная задача до
deployment.

## Share links and guests

Share-link token и гостевая session — разные credentials: token выдаётся один раз
в URL fragment, гостевая cookie имеет Path одной ссылки, а PostgreSQL хранит
только хеши обоих секретов. PIN/password guessing ограничивается Argon2id,
двумя worker permits и сериализованным per-link счётчиком. Успешный гостевой
principal привязан к одной книге и роли. Owner revoke и deadline меняют доступ
через delivery gate и закрывают idle WebSocket; гарантии пока ограничены одним
процессом. Distributed invalidation и production owner authentication остаются
deployment gates.

Comments имеют отдельную revision и шифруются как записи хранения. Они не
попадают в canonical workbook history/hash; адрес используют stable IDs ячейки.
Список и mutation проверяют owner/member или guest capability в PostgreSQL,
ограничивают размер ответа и не пишут текст комментария в tracing.

## Revocation и in-flight bytes

ACL transaction сама по себе не очищает socket. Control lane блокирует новый send под old epoch, затем commit ACL/control event, удаляет queued payloads, требует reauthorization. Уже переданные socket bytes невозможно отозвать. Граница linearization и lock order описаны в collaboration.md; privacy claim ограничен новыми sends после revoke boundary.

Membership revocation охватывает все активные документы пользователя. Offline catch-up и idempotent receipt также требуют актуальных прав. Будущий несколько-process deployment требует fenced/acknowledged invalidation; ненадежный pub/sub сам по себе не решает это.

## Логи, секреты, наблюдаемость

Разрешенные tracing fields: opaque IDs, document versions, codes, durations и bounded counts. Запрещены cell values, formula source, comments, clipboard, imported personal names, tokens/cookies, connection secrets. Не логировать raw WS frame, SQL bind values или entire errors с пользовательским payload. Formula errors агрегируются по code.

Секреты поступают через env/file provider и redacted error handling. OTel adapter отключаемый, external export по умолчанию выключен. Metrics labels ограничены; document_id не превращается в unbounded metric label. Audit отдельно от обычных логов: ACL/export metadata без содержимого документов.

TLS termination boundary предусмотрена, reverse proxy добавляется по необходимости. CSP/security headers и exact development origin settings не должны превращаться в wildcard production policy. Нет обязательных зарубежных SaaS. Retention конфигурируется для документов, audit и blobs; offboarding deletion не маскируется snapshot GC.

## Production exit criteria

До production нужны auth/CSRF/session review, tenant isolation tests, ACL revoke race tests, dependency audit, backup/PITR restore drill, resource-limit/load evidence, retention policy и доступная operator процедура quarantined recovery. Это будущие gate requirements, не основание раздувать phase 0 инфраструктуру.
