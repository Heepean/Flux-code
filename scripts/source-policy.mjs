import { readdir, readFile, lstat, stat } from 'node:fs/promises'
import path from 'node:path'
import ts from 'typescript'

export const localeCodes = ['ru', 'en']

const excludedDirectories = new Set([
  '.git',
  '.vite',
  'build',
  'coverage',
  'dist',
  'fixtures',
  'node_modules',
  'target',
  'tests',
])

const sourceExtensions = new Set([
  '.cjs',
  '.js',
  '.jsx',
  '.mjs',
  '.mts',
  '.ps1',
  '.psm1',
  '.rs',
  '.ts',
  '.tsx',
])

const forbiddenWords = [
  ['TO', 'DO'].join(''),
  ['FIX', 'ME'].join(''),
  ['un', 'implemented!'].join(''),
  ['to', 'do!'].join(''),
  ['coming', 'soon'].join(' '),
  ['lorem', 'ipsum'].join(' '),
]

const forbiddenPatterns = forbiddenWords.map((word) => {
  const escapedWord = word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  return {
    word,
    expression: new RegExp(`(?:^|[^A-Za-z0-9_])${escapedWord}(?=$|[^A-Za-z0-9_])`, 'i'),
  }
})


function addIssue(issues, code, fields = {}) {
  issues.push({ code, ...fields })
}

function flattenResource(value, prefix, output, issues, locale) {
  if (typeof value === 'string') {
    output.set(prefix, value)
    return
  }

  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    for (const [key, nestedValue] of Object.entries(value)) {
      const nextKey = prefix ? `${prefix}.${key}` : key
      flattenResource(nestedValue, nextKey, output, issues, locale)
    }
    return
  }

  addIssue(issues, 'NON_STRING_VALUE', { locale, key: prefix })
}

function isTranslationCall(expression) {
  if (ts.isIdentifier(expression)) {
    return expression.text === 't'
  }
  if (ts.isPropertyAccessExpression(expression)) {
    return expression.name.text === 't'
  }
  return false
}

function isFluxTranslateCall(expression, sourceFile) {
  if (!ts.isIdentifier(expression) || expression.text !== 'translate') return false
  return sourceFile.fileName.replaceAll('\\', '/').endsWith('/src/App.tsx')
}

function collectTranslationReferences(sourceFile, relativePath, issues) {
  const keys = new Set()

  function visit(node) {
    if (ts.isCallExpression(node) && isTranslationCall(node.expression) && !isFluxTranslateCall(node.expression, sourceFile)) {
      const argument = node.arguments[0]
      if (argument && (ts.isStringLiteral(argument) || ts.isNoSubstitutionTemplateLiteral(argument))) {
        keys.add(argument.text)
      } else {
        addIssue(issues, 'DYNAMIC_KEY', {
          file: relativePath,
          line: sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1,
        })
      }
    }

    if (ts.isCallExpression(node) && isFluxTranslateCall(node.expression, sourceFile) && node.arguments.length >= 2) {
      const keyArgument = node.arguments[1]
      if (ts.isStringLiteral(keyArgument) || ts.isNoSubstitutionTemplateLiteral(keyArgument)) {
        keys.add(keyArgument.text)
      } else if (!(ts.isIdentifier(keyArgument) && keyArgument.text === 'key')) {
        addIssue(issues, 'DYNAMIC_KEY', {
          file: relativePath,
          line: sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1,
        })
      }
    }

    ts.forEachChild(node, visit)
  }

  visit(sourceFile)
  return keys
}

function scriptKindFor(filePath) {
  const extension = path.extname(filePath).toLowerCase()
  if (extension === '.tsx') return ts.ScriptKind.TSX
  if (extension === '.jsx') return ts.ScriptKind.JSX
  if (extension === '.js' || extension === '.mjs' || extension === '.cjs') return ts.ScriptKind.JS
  return ts.ScriptKind.TS
}

async function collectFiles(roots, issues) {
  const files = []
  const seenFiles = new Set()

  async function visitDirectory(root, current) {
    let entries
    try {
      entries = await readdir(current, { withFileTypes: true })
    } catch (error) {
      addIssue(issues, 'SOURCE_READ_ERROR', {
        file: path.relative(root, current),
        message: error instanceof Error ? error.message : String(error),
      })
      return
    }

    for (const entry of entries) {
      if (entry.isSymbolicLink()) continue
      if (entry.isDirectory()) {
        if (excludedDirectories.has(entry.name.toLowerCase())) continue
        await visitDirectory(root, path.join(current, entry.name))
        continue
      }
      if (!entry.isFile() || !sourceExtensions.has(path.extname(entry.name).toLowerCase())) continue
      const filePath = path.join(current, entry.name)
      if (seenFiles.has(filePath)) continue
      seenFiles.add(filePath)
      files.push({ filePath, relativePath: path.relative(root, filePath) })
    }
  }

  for (const root of roots) {
    let rootStats
    try {
      rootStats = await lstat(root)
    } catch (error) {
      addIssue(issues, 'SOURCE_ROOT_ERROR', {
        file: root,
        message: error instanceof Error ? error.message : String(error),
      })
      continue
    }
    if (!rootStats.isDirectory() || rootStats.isSymbolicLink()) {
      addIssue(issues, 'SOURCE_ROOT_INVALID', { file: root })
      continue
    }
    await visitDirectory(root, root)
  }

  return files
}

async function readSourceFile(file, issues, maxSourceBytes) {
  try {
    const fileStats = await stat(file.filePath)
    if (fileStats.size > maxSourceBytes) {
      addIssue(issues, 'SOURCE_TOO_LARGE', { file: file.relativePath, size: fileStats.size })
      return null
    }
    return await readFile(file.filePath, { encoding: 'utf8' })
  } catch (error) {
    addIssue(issues, 'SOURCE_READ_ERROR', {
      file: file.relativePath,
      message: error instanceof Error ? error.message : String(error),
    })
    return null
  }
}

export async function validateLocales(localeDirectory, sourceRoots) {
  const issues = []
  const resources = new Map()

  for (const locale of localeCodes) {
    const filePath = path.join(localeDirectory, `${locale}.json`)
    let source
    try {
      source = await readFile(filePath, { encoding: 'utf8' })
    } catch (error) {
      addIssue(issues, error?.code === 'ENOENT' ? 'MISSING_LOCALE' : 'LOCALE_READ_ERROR', {
        locale,
        file: filePath,
        message: error instanceof Error ? error.message : String(error),
      })
      continue
    }

    let document
    try {
      document = JSON.parse(source)
    } catch (error) {
      addIssue(issues, 'INVALID_JSON', {
        locale,
        file: filePath,
        message: error instanceof Error ? error.message : String(error),
      })
      continue
    }

    if (document === null || typeof document !== 'object' || Array.isArray(document)) {
      addIssue(issues, 'INVALID_RESOURCE_ROOT', { locale, file: filePath })
      continue
    }

    const flattened = new Map()
    flattenResource(document, '', flattened, issues, locale)
    resources.set(locale, flattened)
  }

  const english = resources.get('en')
  if (!english) {
    addIssue(issues, 'REFERENCE_LOCALE_INVALID', { locale: 'en' })
    return issues
  }

  const sourceFiles = await collectFiles(sourceRoots, issues)
  const referencedKeys = new Set()
  for (const file of sourceFiles) {
    if (file.relativePath.toLowerCase().startsWith('tests/')) continue
    if (file.relativePath.toLowerCase().includes('/tests/') || file.relativePath.toLowerCase().startsWith('tests/')) continue
    const source = await readSourceFile(file, issues, 4 * 1024 * 1024)
    if (source === null) continue
    const sourceFile = ts.createSourceFile(
      file.filePath,
      source,
      ts.ScriptTarget.Latest,
      true,
      scriptKindFor(file.filePath),
    )

    for (const diagnostic of sourceFile.parseDiagnostics) {
      addIssue(issues, 'SOURCE_SYNTAX_ERROR', {
        file: file.relativePath,
        message: ts.flattenDiagnosticMessageText(diagnostic.messageText, '\n'),
      })
    }

    for (const key of collectTranslationReferences(sourceFile, file.relativePath, issues)) {
      referencedKeys.add(key)
      if (!english.has(key)) addIssue(issues, 'UNKNOWN_KEY', { file: file.relativePath, key })
    }
  }

  const englishKeys = new Set(english.keys())
  for (const [locale, resource] of resources) {
    for (const key of englishKeys) {
      if (!resource.has(key)) addIssue(issues, 'MISSING_KEY', { locale, key })
    }
    for (const key of resource.keys()) {
      if (!englishKeys.has(key)) addIssue(issues, 'EXTRA_KEY', { locale, key })
    }
  }
  for (const key of englishKeys) {
    if (!referencedKeys.has(key)) addIssue(issues, 'UNUSED_KEY', { locale: 'en', key })
  }

  return issues
}

function scanText(filePath, relativePath, source, issues) {
  for (const { word, expression } of forbiddenPatterns) {
    if (expression.test(source)) {
      addIssue(issues, 'FORBIDDEN_TOKEN', { file: relativePath, token: word })
    }
  }

  if (path.extname(filePath).toLowerCase() === '.rs') {
    const productionSource = source
      .replace(/#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\][\s\S]*$/m, '')
      .replace(/#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\][\s\S]*?\n\s*}\s*/g, '')
    const panicCall = /\.\s*(unwrap|expect)\s*\(/g
    for (const match of productionSource.matchAll(panicCall)) {
      const line = productionSource.slice(0, match.index).split('\n').length
      addIssue(issues, 'RUST_UNWRAP', { file: relativePath, member: match[1], line })
    }
    return
  }

  if (['.ts', '.tsx', '.mts', '.cts', '.js', '.jsx', '.mjs', '.cjs'].includes(path.extname(filePath).toLowerCase())) {
    const sourceFile = ts.createSourceFile(filePath, source, ts.ScriptTarget.Latest, true, scriptKindFor(filePath))
    function visit(node) {
      if (node.kind === ts.SyntaxKind.AnyKeyword) {
        addIssue(issues, 'TYPESCRIPT_ANY', {
          file: relativePath,
          line: sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile)).line + 1,
        })
      }
      ts.forEachChild(node, visit)
    }
    visit(sourceFile)
  }
}

export async function scanForbiddenSources(sourceRoots, options = {}) {
  const issues = []
  const maxSourceBytes = options.maxSourceBytes ?? 4 * 1024 * 1024
  const sourceFiles = await collectFiles(sourceRoots, issues)

  for (const file of sourceFiles) {
    const source = await readSourceFile(file, issues, maxSourceBytes)
    if (source !== null) scanText(file.filePath, file.relativePath, source, issues)
  }

  return issues
}
