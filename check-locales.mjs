import { fileURLToPath } from 'node:url'
import { validateLocales } from './scripts/source-policy.mjs'

const issues = await validateLocales(fileURLToPath(new URL('./src/locales/', import.meta.url)), [fileURLToPath(new URL('./src/', import.meta.url))])
if (issues.length > 0) {
  for (const issue of issues) process.stderr.write(`${issue.code}: ${JSON.stringify(issue)}\n`)
  process.exitCode = 1
} else {
  process.stdout.write('Locale resources match the en/ru source keys.\n')
}
