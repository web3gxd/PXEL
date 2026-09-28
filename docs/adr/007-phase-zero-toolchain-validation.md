# ADR-007: совместимость toolchain фазы 0

Статус: принято для локального skeleton фазы 0; Linux/remote CI verification pending.

## Контекст

Ранние версии были individual release candidates. Полная сборка должна проверяться
до объявления tested baseline. Исходный кандидат ESLint 10 несовместим с peer range
стабильного Babel ESLint parser; typescript-eslint 8.71 исключает TypeScript 7.

## Решение

Сохраняем TS 7.0.2 strict, React 19.3.0, Vite 8.3.1, pnpm 12.6.0. Для syntax/core
lint используем ESLint 9.39.5, Babel core 7.29.7, parser 7.29.9, TS/JSX syntax
plugins 7.29.7. Type checking выполняется отдельным `tsc --noEmit`; type-aware
ESLint rules не заявляются. Никакого force/peer override. Deprecated ESLint 9
является временным исключением LINT-COMPAT с критерием удаления в
[TOOLING.md](../../apps/web/TOOLING.md).

Локально совместно проверяются Node 24.18.0, Rust 1.98.1 GNU, SQLx 0.9.0,
PostgreSQL 18.6, Prost 0.14.4, protobuf-es 2.15.0, Buf 1.73.0. CI закрепляет
эти версии и PostgreSQL image digest. Node 24.21 из раннего предложения остается
непроверенным upgrade candidate; более старая установленная 24.18 используется
явно, без утверждения, что это latest patch. MSRV policy не вводится.

Rust/TypeScript protobuf используют один descriptor и общий handshake fixture.
Cargo проверяет domain/wire для wasm32-unknown-unknown, но формульного WASM parity
в фазе 0 нет. SQLx metadata генерируется из реальной PostgreSQL и хранится отдельно.

## Последствия и миграция

Вместо автоматического major update обновление проходит тот же combined suite.
Linux CI пока нельзя назвать зеленым: локальная машина не имеет Docker/WSL,
remote repository не настроен. Это явный незакрытый acceptance gate.
После первого успешного CI обновить статус, не переписывая историю испытаний.
Установки в `.tools/` и `.local/` не входят в source repository.

Источники релизов/peer ranges: npm registry metadata для закрепленных пакетов,
[Babel ESLint parser](https://babeljs.io/docs/babel-eslint-parser),
[typescript-eslint dependency policy](https://typescript-eslint.io/users/dependency-versions),
[Rust releases](https://releases.rs/), [PostgreSQL releases](https://www.postgresql.org/support/versioning/).
