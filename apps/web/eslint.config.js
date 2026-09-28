import js from '@eslint/js';
import globals from 'globals';
import babelParser from '@babel/eslint-parser';

export default [
  {
    ignores: ['dist/**', 'src/generated/**', 'playwright-report/**', 'test-results/**'],
  },
  js.configs.recommended,
  {
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
  },
  {
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      parser: babelParser,
      parserOptions: {
        requireConfigFile: false,
        babelOptions: {
          babelrc: false,
          configFile: false,
          presets: [['@babel/preset-typescript', { allExtensions: true, isTSX: true }]],
          plugins: ['@babel/plugin-syntax-jsx'],
        },
      },
    },
    // TypeScript 7 performs these checks; core ESLint does not understand type namespaces.
    rules: { 'no-undef': 'off', 'no-unused-vars': 'off' },
  },
];
