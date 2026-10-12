// Lint for the UI, kept to the mistakes that break the app at runtime rather
// than to style: a variable read before its declaration ran (New run's "Run
// again" died on one), an undefined name, a Svelte construct used wrong.
// `npm run check` runs it with svelte-check.
import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';

export default [
  { ignores: ['dist/', 'node_modules/'] },
  js.configs.recommended,
  ...svelte.configs['flat/recommended'],
  {
    languageOptions: {
      ecmaVersion: 2024,
      sourceType: 'module',
      // __UI_VERSION__: the package version, put in by Vite (vite.config.js define)
      globals: { ...globals.browser, ...globals.node, __UI_VERSION__: 'readonly' },
    },
    rules: {
      // Only what runs straight away: a read inside a function or a $derived
      // happens later, once the whole script has run.
      'no-use-before-define': ['error', { functions: false, classes: false, variables: false }],
      'no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrors: 'none' }],
      // Style and render speed, not breakage: left out so the check stays
      // about what stops the app from working.
      'svelte/require-each-key': 'off',
      'svelte/prefer-svelte-reactivity': 'off',
    },
  },
];
