import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, it } from 'vitest'
import { scanForbiddenSources, validateLocales } from '../../scripts/source-policy.mjs'

const localeCodes = ['ru', 'en']

let fixtureRoot
let localeDirectory
let sourceDirectory
let externalSourceDirectory

async function createFixture(resources = { common: { send: 'Send' } }) {
  fixtureRoot = await mkdtemp(join(tmpdir(), 'flux-source-policy-'))
  localeDirectory = join(fixtureRoot, 'locales')
  sourceDirectory = join(fixtureRoot, 'src')
  await mkdir(localeDirectory, { recursive: true })
  await mkdir(sourceDirectory, { recursive: true })

  for (const language of localeCodes) {
    await writeFile(join(localeDirectory, `${language}.json`), JSON.stringify(resources), 'utf8')
  }
  await writeFile(join(sourceDirectory, 'App.tsx'), "export const label = t('common.send')\n", 'utf8')
}

beforeEach(async () => {
  await createFixture()
})

afterEach(async () => {
  if (fixtureRoot) await rm(fixtureRoot, { recursive: true, force: true })
  if (externalSourceDirectory) await rm(externalSourceDirectory, { recursive: true, force: true })
})

describe('locale resources', () => {
  it('accepts all configured locales when keys match and are referenced', async () => {
    const issues = await validateLocales(localeDirectory, [sourceDirectory])
    assert.deepEqual(issues, [])
  })

  it('reports a missing locale file', async () => {
    await rm(join(localeDirectory, 'ru.json'))
    const issues = await validateLocales(localeDirectory, [sourceDirectory])
    assert.ok(issues.some((issue) => issue.code === 'MISSING_LOCALE' && issue.locale === 'ru'))
  })

  it('reports missing and extra keys against the English resource', async () => {
    await writeFile(join(localeDirectory, 'ru.json'), JSON.stringify({ common: { send: 'Отправить', cancel: 'Отмена' } }), 'utf8')
    await writeFile(join(localeDirectory, 'en.json'), JSON.stringify({ common: { send: 'Send', stop: 'Stop' } }), 'utf8')
    const issues = await validateLocales(localeDirectory, [sourceDirectory])
    assert.ok(issues.some((issue) => issue.code === 'MISSING_KEY' && issue.locale === 'ru' && issue.key === 'common.stop'))
    assert.ok(issues.some((issue) => issue.code === 'EXTRA_KEY' && issue.locale === 'ru' && issue.key === 'common.cancel'))
  })

  it('reports keys that are not referenced by source', async () => {
    for (const language of localeCodes) {
      await writeFile(
        join(localeDirectory, `${language}.json`),
        JSON.stringify({ common: { send: 'Send', unused: 'Unused copy' } }),
        'utf8',
      )
    }
    const issues = await validateLocales(localeDirectory, [sourceDirectory])
    assert.ok(issues.some((issue) => issue.code === 'UNUSED_KEY' && issue.key === 'common.unused'))
  })

  it('reports unknown and dynamic translation keys', async () => {
    await writeFile(join(sourceDirectory, 'App.tsx'), "const first = t('common.unknown'); const second = t(labelKey)\n", 'utf8')
    const issues = await validateLocales(localeDirectory, [sourceDirectory])
    assert.ok(issues.some((issue) => issue.code === 'UNKNOWN_KEY' && issue.key === 'common.unknown'))
    assert.ok(issues.some((issue) => issue.code === 'DYNAMIC_KEY'))
  })

  it('reports malformed JSON and non-string resource leaves', async () => {
    await writeFile(join(localeDirectory, 'ru.json'), '{ broken', 'utf8')
    await writeFile(join(localeDirectory, 'en.json'), JSON.stringify({ common: { send: 7 } }), 'utf8')
    const issues = await validateLocales(localeDirectory, [sourceDirectory])
    assert.ok(issues.some((issue) => issue.code === 'INVALID_JSON' && issue.locale === 'ru'))
    assert.ok(issues.some((issue) => issue.code === 'NON_STRING_VALUE' && issue.locale === 'en'))
  })
})

describe('forbidden source scan', () => {
  it('finds prohibited source tokens and TypeScript any syntax', async () => {
    await writeFile(join(sourceDirectory, 'unsafe.ts'), "const value: any = 'TODO';\n// FIXME\n", 'utf8')
    await writeFile(join(sourceDirectory, 'unsafe.rs'), 'fn value() { option.unwrap(); }\n', 'utf8')
    const issues = await scanForbiddenSources([sourceDirectory])
    assert.ok(issues.some((issue) => issue.code === 'FORBIDDEN_TOKEN' && issue.token === 'TODO'))
    assert.ok(issues.some((issue) => issue.code === 'TYPESCRIPT_ANY'))
    assert.ok(issues.some((issue) => issue.code === 'RUST_UNWRAP'))
  })

  it('ignores test and fixture trees but still scans production source', async () => {
    const testsDirectory = join(fixtureRoot, 'tests')
    const fixtureDirectory = join(fixtureRoot, 'fixtures')
    await mkdir(testsDirectory)
    await mkdir(fixtureDirectory)
    await writeFile(join(testsDirectory, 'sample.ts'), "const sample = 'TODO'\n", 'utf8')
    await writeFile(join(fixtureDirectory, 'sample.rs'), 'value.expect("test")\n', 'utf8')
    await writeFile(join(sourceDirectory, 'App.tsx'), "const copy = 'coming soon'\n", 'utf8')
    const issues = await scanForbiddenSources([fixtureRoot])
    assert.ok(issues.some((issue) => issue.code === 'FORBIDDEN_TOKEN' && issue.token === 'coming soon'))
    assert.equal(issues.some((issue) => issue.file.includes('tests') || issue.file.includes('fixtures')), false)
  })

  it('ignores forbidden words inside ordinary identifiers and placeholder attributes', async () => {
    await writeFile(join(sourceDirectory, 'App.tsx'), "const todoList = []; const view = <input placeholder=\"Choose one\" />\n", 'utf8')
    const issues = await scanForbiddenSources([sourceDirectory])
    assert.deepEqual(issues, [])
  })

  it('does not follow symlinked source directories', async () => {
    externalSourceDirectory = await mkdtemp(join(tmpdir(), 'flux-linked-source-'))
    await writeFile(join(externalSourceDirectory, 'unsafe.ts'), "const copy = 'TODO'\n", 'utf8')
    await symlink(externalSourceDirectory, join(sourceDirectory, 'linked'), 'junction')
    const issues = await scanForbiddenSources([sourceDirectory])
    assert.deepEqual(issues, [])
  })
})



