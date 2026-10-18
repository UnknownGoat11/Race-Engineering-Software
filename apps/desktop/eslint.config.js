// ESLint flat config: typescript-eslint (type-checked) + React hooks rules.
import js from '@eslint/js';
import { defineConfig, globalIgnores } from 'eslint/config';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import globals from 'globals';
import tseslint from 'typescript-eslint';

const inlineStylesPlugin = {
  rules: {
    'no-inline-styles': {
      meta: {
        type: 'suggestion',
        docs: {
          description: 'Forbid inline style props except allowlisted --dyn-* custom properties',
        },
        messages: {
          noInlineStyle: 'Inline style props are forbidden. Use CSS classes or --dyn-* variables instead.',
        },
        schema: [],
      },
      create(context) {
        return {
          JSXAttribute(node) {
            if (node.name && node.name.name === 'style') {
              if (
                node.value &&
                node.value.type === 'JSXExpressionContainer' &&
                node.value.expression.type === 'ObjectExpression'
              ) {
                const props = node.value.expression.properties;
                const allAllowlisted =
                  props.length > 0 &&
                  props.every((p) => {
                    if (p.type !== 'Property') return false;
                    const key = p.key;
                    const name =
                      key.type === 'Identifier' ? key.name : key.type === 'Literal' ? String(key.value) : '';
                    return name.startsWith('--dyn-') || name === '--dyn';
                  });
                if (!allAllowlisted) {
                  context.report({
                    node,
                    messageId: 'noInlineStyle',
                  });
                }
              } else if (node.value) {
                context.report({
                  node,
                  messageId: 'noInlineStyle',
                });
              }
            }
          },
        };
      },
    },
  },
};

export default defineConfig([
  globalIgnores(['dist', 'coverage', 'src-tauri', 'node_modules']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.strictTypeChecked,
      tseslint.configs.stylisticTypeChecked,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      '@typescript-eslint/restrict-template-expressions': ['error', { allowNumber: true }],
      '@typescript-eslint/consistent-type-imports': 'error',
    },
  },
  {
    files: [
      'src/graph/**/*.{ts,tsx}',
      'src/components/telemetry/ChannelSelectorModal.tsx',
    ],
    plugins: {
      'custom-rules': inlineStylesPlugin,
    },
    rules: {
      'custom-rules/no-inline-styles': 'warn',
    },
  },
  {
    files: ['*.js'],
    extends: [js.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: globals.node,
    },
  },
]);

