# Vendored dependency license texts

`@bufbuild/protobuf` 2.15.0 declares `(Apache-2.0 AND BSD-3-Clause)` but its npm
package does not include standalone license files. The browser-notice generator
uses these version-specific texts:

- `protobuf-es-2.15.0-APACHE.txt`: the unchanged upstream
  [v2.15.0 LICENSE](https://github.com/bufbuild/protobuf-es/blob/v2.15.0/LICENSE).
- `protobuf-es-2.15.0-BSD.txt`: the Google copyright and complete BSD conditions
  from the installed package's `dist/esm/wire/varint.js` comment, also present in
  [upstream varint.ts](https://github.com/bufbuild/protobuf-es/blob/v2.15.0/packages/protobuf/src/wire/varint.ts).

These files retain their upstream licenses. Regenerate and review notices when
updating dependencies; do not apply a version-specific fallback to a newer package.
