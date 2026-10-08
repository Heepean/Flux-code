import { fileURLToPath } from 'node:url'
import { scanForbiddenSources } from './scripts/source-policy.mjs'

const root = fileURLToPath(new URL('./src/', import.meta.url))
const issues = await scanForbiddenSources([root])
if (issues.length > 0) {
  for (const issue of issues) process.stderr.write(`${issue.code}: ${JSON.stringify(issue)}\n`)
  process.exitCode = 1
} else {
  process.stdout.write('Source policy passed for src/.\n')
}
