# Frontend verification

TypeScript 7 strict type checking runs separately from ESLint syntax/core rules.
At bootstrap, typescript-eslint 8.71.0 supports TypeScript below 6.1, and
@babel/eslint-parser 7.29.9 supports ESLint through 9. We therefore pin ESLint
9.39.5 with Babel 7.29.7 and its 7.29.9 parser. No peer constraints are overridden.
The dependencies use MIT licenses; TypeScript and Playwright use Apache-2.0;
protobuf-es uses Apache-2.0 AND BSD-3-Clause.

**LINT-COMPAT (temporary exception):** the registry marks ESLint 9.39.5
deprecated. The stable Babel ESLint parser currently declares support only
through ESLint 9, while the stable typescript-eslint parser excludes TypeScript 7. This pin keeps the combined toolchain within its declared compatibility
contracts. Upgrade to ESLint 10 when a stable parser declares support for both
that ESLint major and the project TypeScript syntax; remove this exception after
strict type checking, lint, unit tests and production build pass together.

`pnpm --filter @pxel/web test:e2e` expects the real dev backend and Vite to run at
localhost:8080 / localhost:5173. It does not substitute mocked authentication or
WebSocket persistence. It exercises phase-0 handshake and Canvas only.

`pxel:grid-ready` is a browser performance mark after the initial Canvas paint
command. Playwright attaches this observation, viewport, DPR, user agent and DOM
count to each run; shared CI does not enforce a machine-specific latency limit.
The root testing document defines the reference-runner performance protocol.
