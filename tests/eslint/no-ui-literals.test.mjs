import { RuleTester } from 'eslint'
import { describe, it } from 'vitest'
import tseslint from 'typescript-eslint'
import noUiLiterals from '../../scripts/eslint/no-ui-literals.mjs'

RuleTester.describe = describe
RuleTester.it = it

const ruleTester = new RuleTester({
  languageOptions: {
    ecmaVersion: 'latest',
    sourceType: 'module',
    parser: tseslint.parser,
    parserOptions: { ecmaFeatures: { jsx: true } },
  },
})

ruleTester.run('no-ui-literals', noUiLiterals, {
  valid: [
    "const view = <button>{t('send')}</button>",
    "const view = <input aria-label={t('search')} />",
    "const view = <span>{status === 'busy' ? t('busy') : t('ready')}</span>",
    "const view = <span>{hasMessage && t('message')}</span>",
    { filename: 'src/StatusLabel.tsx', code: "type State = 'busy' | 'ready'; const label = ({ state }: { state: State }) => <span>{state}</span>" },
    'const view = <section className={"panel"} />',
    '<section className="panel" id="workspace" role="main" />',
    'const view = <div>   </div>',
  ],
  invalid: [
    {
      code: '<button>Send</button>',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      filename: 'src/Button.tsx',
      code: 'const Button = () => <button>Send</button>',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: '<input aria-label="Search chats" />',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: '<input placeholder="Type a message" />',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: '<img alt="Flux icon" />',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: '<input aria-label={"Search"} />',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: '<p>{"Hardcoded"}</p>',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: '<p>{`Hardcoded template`}</p>',
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: `<span>{hasMessage && 'Message'}</span>`,
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
    {
      code: `<span>{status === 'busy' ? 'Busy' : 'Ready'}</span>`,
      errors: [{ messageId: 'noVisibleLiteral' }],
    },
  ],
})
