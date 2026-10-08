import { useEffect, useMemo, useState, type ReactNode } from 'react'
import Editor from '@monaco-editor/react'
import { invoke } from '@tauri-apps/api/core'
import type { Monaco } from '@monaco-editor/react'
import { useRef } from 'react'
import type { editor as MonacoEditor } from 'monaco-editor'
import { TerminalPanel } from './TerminalPanel'
import { DebugPanel } from './DebugPanel'
import { fileUri, LanguageClient } from './languageClient'
import editorWorker from '../node_modules/monaco-editor/esm/vs/editor/editor.worker.js?worker'
import jsonWorker from '../node_modules/monaco-editor/esm/vs/language/json/json.worker.js?worker'
import cssWorker from '../node_modules/monaco-editor/esm/vs/language/css/css.worker.js?worker'
import htmlWorker from '../node_modules/monaco-editor/esm/vs/language/html/html.worker.js?worker'
import tsWorker from '../node_modules/monaco-editor/esm/vs/language/typescript/ts.worker.js?worker'

interface WorkspaceEntry {
  name: string
  path: string
  is_directory: boolean
}

interface WorkspaceProps {
  projectPath: string
  language: 'en' | 'ru'
  onChooseProject: () => void
}

interface OpenDocument {
  path: string
  content: string
  savedContent: string
}

interface SearchMatch {
  path: string
  line: number | null
  preview: string
  file_name_match: boolean
}

interface SearchPage {
  matches: SearchMatch[]
  total: number
  offset: number
  page_size: number
}

interface GitChange {
  path: string
  indexStatus: string
  worktreeStatus: string
  untracked: boolean
}

interface EditorProblem {
  path: string
  line: number
  column: number
  severity: number
  message: string
}

const labels = {
  en: {
    explorer: 'EXPLORER', openFolder: 'Open Folder', newFile: 'New File', newFolder: 'New Folder', refresh: 'Refresh',
    search: 'Search files and contents', searchPlaceholder: 'Search in project', includeGenerated: 'Include generated folders',
    rename: 'Rename', delete: 'Delete', deleteConfirm: 'Delete this item? This action cannot be undone.',
    previousPage: 'Previous page', nextPage: 'Next page', searchCount: '{{start}}–{{end}} of {{total}}', noSearchResults: 'No matches found',
    save: 'Save', saved: 'Saved', unsaved: 'Unsaved changes', selectFile: 'Select a file to start editing', closeFile: 'Close {{name}}', saveShortcut: 'Ctrl + S', discardChanges: 'Discard unsaved changes?',
    noFolder: 'Open a folder to start working', loading: 'Loading…', loadError: 'Could not load folder',
    fileName: 'Enter a file name relative to the project',
    terminal: 'Terminal',
    git: 'Source Control', commit: 'Commit', commitPlaceholder: 'Commit message', stage: 'Stage', unstage: 'Unstage', refreshGit: 'Refresh source control', noGitChanges: 'No changes detected', untrackedStatus: 'U', stageSymbol: '+', unstageSymbol: '−',
    connectLanguageServer: 'Connect language server', stopLanguageServer: 'Stop language server', languageServerCommand: 'Language server command', languageServerArguments: 'Arguments as a JSON array', languageServerSetup: 'Language server setup (language, command, JSON args)', formatDocument: 'Format document', goToDefinition: 'Go to definition', formatLabel: 'Format', goToDefinitionTitle: 'Go to definition (F12)', formatTitle: 'Format document (Shift+Alt+F)', problems: 'Problems', noProblems: 'No problems detected', problemCount: 'Problems ({{count}})', problemLocation: '{{path}} · line {{line}}, column {{column}}', languageServerShort: 'LSP', debug: 'Run and debug', debugShort: 'DBG',
  },
  ru: {
    explorer: 'ПРОВОДНИК', openFolder: 'Открыть папку', newFile: 'Новый файл', newFolder: 'Новая папка', refresh: 'Обновить',
    search: 'Поиск файлов и содержимого', searchPlaceholder: 'Поиск в проекте', includeGenerated: 'Искать в сгенерированных папках',
    rename: 'Переименовать', delete: 'Удалить', deleteConfirm: 'Удалить этот элемент? Это действие нельзя отменить.',
    previousPage: 'Предыдущая страница', nextPage: 'Следующая страница', searchCount: '{{start}}–{{end}} из {{total}}', noSearchResults: 'Совпадений не найдено',
    save: 'Сохранить', saved: 'Сохранено', unsaved: 'Есть несохранённые изменения', selectFile: 'Выберите файл для редактирования', closeFile: 'Закрыть {{name}}', saveShortcut: 'Ctrl + S', discardChanges: 'Отменить несохранённые изменения?',
    noFolder: 'Откройте папку проекта', loading: 'Загрузка…', loadError: 'Не удалось открыть папку',
    fileName: 'Введите имя файла относительно папки проекта',
    terminal: 'Терминал',
    git: 'Контроль версий', commit: 'Зафиксировать', commitPlaceholder: 'Сообщение коммита', stage: 'Подготовить', unstage: 'Снять подготовку', refreshGit: 'Обновить Git', noGitChanges: 'Изменений нет', untrackedStatus: 'U', stageSymbol: '+', unstageSymbol: '−',
    connectLanguageServer: 'Подключить языковой сервер', stopLanguageServer: 'Отключить языковой сервер', languageServerCommand: 'Команда языкового сервера', languageServerArguments: 'Аргументы в формате JSON-массива', languageServerSetup: 'Настройка сервера (язык, команда, аргументы JSON)', formatDocument: 'Форматировать документ', goToDefinition: 'Перейти к определению', formatLabel: 'Форматировать', goToDefinitionTitle: 'Перейти к определению (F12)', formatTitle: 'Форматировать документ (Shift+Alt+F)', problems: 'Проблемы', noProblems: 'Ошибок не обнаружено', problemCount: 'Проблемы ({{count}})', problemLocation: '{{path}} · строка {{line}}, столбец {{column}}', languageServerShort: 'LSP', debug: 'Запуск и отладка', debugShort: 'DBG',
  },
} as const

const workspaceStateKey = 'flux-code-workspace-state:'

const monacoScope = globalThis as typeof globalThis & {
  MonacoEnvironment?: { getWorker: (_moduleId: string, label: string) => Worker }
}
monacoScope.MonacoEnvironment ??= {
  getWorker: (_moduleId, label) => {
    if (label === 'json') return new jsonWorker()
    if (label === 'css' || label === 'scss' || label === 'less') return new cssWorker()
    if (label === 'html' || label === 'handlebars' || label === 'razor') return new htmlWorker()
    if (label === 'typescript' || label === 'javascript') return new tsWorker()
    return new editorWorker()
  },
}

function languageForFile(path: string) {
  const extension = path.split('.').at(-1)?.toLowerCase()
  const languages: Record<string, string> = {
    ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
    json: 'json', html: 'html', htm: 'html', css: 'css', scss: 'scss', less: 'less', md: 'markdown',
    py: 'python', rs: 'rust', toml: 'toml', yaml: 'yaml', yml: 'yaml', xml: 'xml', sql: 'sql',
    sh: 'shell', ps1: 'powershell', bat: 'bat', cmd: 'bat', c: 'c', h: 'c', cpp: 'cpp', hpp: 'cpp',
    java: 'java', kt: 'kotlin', go: 'go', rb: 'ruby', php: 'php', cs: 'csharp', vue: 'html', svelte: 'html',
  }
  return languages[extension ?? ''] ?? 'plaintext'
}

function fileIcon(name: string) {
  const extension = name.split('.').at(-1)?.toLowerCase()
  if (['ts', 'tsx', 'js', 'jsx'].includes(extension ?? '')) return 'TS'
  if (['rs', 'py', 'go', 'java', 'c', 'cpp'].includes(extension ?? '')) return '</>'
  if (['json', 'toml', 'yaml', 'yml'].includes(extension ?? '')) return '{}'
  if (['md', 'txt'].includes(extension ?? '')) return '≡'
  return '·'
}

function closeFileLabel(template: string, path: string) {
  return template.replace(/\{\{name\}\}/, path)
}

function problemCountLabel(template: string, count: number) {
  return template.replace(/\{\{count\}\}/, String(count))
}

function problemLocationLabel(template: string, problem: EditorProblem) {
  return template
    .replace(/\{\{path\}\}/, problem.path)
    .replace(/\{\{line\}\}/, String(problem.line))
    .replace(/\{\{column\}\}/, String(problem.column))
}

function displayPath(path: string) {
  return path.split('/').join(' / ')
}

function searchCountLabel(template: string, start: number, end: number, total: number) {
  return template
    .replace(/\{\{start\}\}/, String(start))
    .replace(/\{\{end\}\}/, String(end))
    .replace(/\{\{total\}\}/, String(total))
}

function searchResultLabel(path: string, line: number | null) {
  return line ? `${path}:${line}` : path
}

const editorOptions = {
  automaticLayout: true,
  minimap: { enabled: true },
  glyphMargin: true,
  fontSize: 13,
  fontFamily: "Consolas, 'Courier New', monospace",
  fontLigatures: true,
  tabSize: 2,
  wordWrap: 'off' as const,
  scrollBeyondLastLine: false,
  smoothScrolling: true,
  renderWhitespace: 'selection' as const,
  padding: { top: 12 },
}
const editorTheme = 'vs-dark'

export function Workspace({ projectPath, language, onChooseProject }: WorkspaceProps) {
  const t = labels[language]
  const [entriesByFolder, setEntriesByFolder] = useState<Record<string, WorkspaceEntry[]>>({})
  const [expanded, setExpanded] = useState<Set<string>>(new Set(['']))
  const [documents, setDocuments] = useState<OpenDocument[]>([])
  const [activePath, setActivePath] = useState('')
  const [error, setError] = useState('')
  const [loadingPath, setLoadingPath] = useState<string | null>(null)
  const [focusedFolder, setFocusedFolder] = useState('')
  const [restoredProjectPath, setRestoredProjectPath] = useState('')
  const [searchQuery, setSearchQuery] = useState('')
  const [searchPage, setSearchPage] = useState<SearchPage | null>(null)
  const [searchOffset, setSearchOffset] = useState(0)
  const [includeGenerated, setIncludeGenerated] = useState(false)
  const [searching, setSearching] = useState(false)
  const [terminalOpen, setTerminalOpen] = useState(false)
  const [debugOpen, setDebugOpen] = useState(false)
  const [breakpoints, setBreakpoints] = useState<Record<string, number[]>>({})
  const [sidebarView, setSidebarView] = useState<'explorer' | 'git' | 'problems'>('explorer')
  const [gitChanges, setGitChanges] = useState<GitChange[]>([])
  const [selectedGitPath, setSelectedGitPath] = useState('')
  const [gitDiff, setGitDiff] = useState('')
  const [commitMessage, setCommitMessage] = useState('')
  const [gitError, setGitError] = useState('')
  const [gitLoading, setGitLoading] = useState(false)
  const [problems, setProblems] = useState<EditorProblem[]>([])
  const [languageServer, setLanguageServer] = useState<LanguageClient | null>(null)
  const [languageServerLanguage, setLanguageServerLanguage] = useState('')
  const [languageServerMessage, setLanguageServerMessage] = useState('')
  const editorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null)
  const monacoRef = useRef<Monaco | null>(null)
  const languageServerRef = useRef<LanguageClient | null>(null)
  const breakpointDecorationIds = useRef<Record<string, string[]>>({})
  const breakpointHandlerRef = useRef<(event: MonacoEditor.IEditorMouseEvent, monaco: Monaco) => void>(() => undefined)
  const goToDefinitionRef = useRef<() => Promise<void>>(async () => undefined)
  const formatActiveDocumentRef = useRef<() => Promise<void>>(async () => undefined)
  const activeDocument = documents.find((document) => document.path === activePath)
  const rootName = projectPath.split(/[\\/]/).filter(Boolean).at(-1) || projectPath

  const connectLanguageServer = async (requestedLanguage?: string) => {
    if (!activeDocument) {
      setLanguageServerMessage(t.selectFile)
      return
    }
    const detectedLanguage = languageForFile(activeDocument.path)
    const languageOptions = ['typescript', 'javascript', 'python', 'rust', 'go', 'json', 'html', 'css']
    const languageId = requestedLanguage ?? (languageOptions.includes(detectedLanguage)
      ? detectedLanguage
      : window.prompt(t.languageServerSetup, 'typescript')?.trim().toLowerCase() ?? '')
    if (!languageId) return
    const configKey = `${workspaceStateKey}${projectPath}:lsp:${languageId}`
    let config: { command?: string; args?: string[] } = {}
    try { config = JSON.parse(localStorage.getItem(configKey) ?? '{}') as typeof config } catch { config = {} }
    const presets: Record<string, { command: string; args: string[] }> = {
      typescript: { command: 'typescript-language-server', args: ['--stdio'] },
      javascript: { command: 'typescript-language-server', args: ['--stdio'] },
      python: { command: 'pyright-langserver', args: ['--stdio'] },
      rust: { command: 'rust-analyzer', args: [] },
      go: { command: 'gopls', args: [] },
      json: { command: 'vscode-json-language-server', args: ['--stdio'] },
      html: { command: 'vscode-html-language-server', args: ['--stdio'] },
      css: { command: 'vscode-css-language-server', args: ['--stdio'] },
    }
    const preset = presets[languageId]
    const savedConfig = { command: config.command ?? preset?.command ?? '', args: config.args ?? preset?.args ?? [] }
    const rawConfig = window.prompt(t.languageServerSetup, `${languageId}; ${savedConfig.command}; ${JSON.stringify(savedConfig.args)}`)
    if (rawConfig === null) return
    const [configuredLanguage, commandText, ...argsText] = rawConfig.split(';')
    const finalLanguage = configuredLanguage?.trim().toLowerCase()
    const command = commandText?.trim()
    if (!finalLanguage || !command) { setLanguageServerMessage(t.languageServerSetup); return }
    let args: unknown
    try { args = JSON.parse(argsText.join(';').trim() || '[]') } catch { setLanguageServerMessage(t.languageServerArguments); return }
    if (!Array.isArray(args) || args.some((argument) => typeof argument !== 'string')) {
      setLanguageServerMessage(t.languageServerArguments)
      return
    }
    localStorage.setItem(`${workspaceStateKey}${projectPath}:lsp:${finalLanguage}`, JSON.stringify({ command, args }))
    setLanguageServerMessage(t.loading)
    try {
      const previous = languageServerRef.current
      languageServerRef.current = null
      setLanguageServer(null)
      setLanguageServerLanguage('')
      await previous?.dispose()
      const client = await LanguageClient.start(
        projectPath,
        finalLanguage,
        command,
        args as string[],
        (uri, values) => {
          const root = projectPath.replaceAll('\\', '/').replace(/\/$/, '')
          let target = uri.startsWith('file:///') ? uri.slice('file:///'.length) : uri
          try { target = decodeURIComponent(target) } catch { /* Preserve the original URI path. */ }
          target = target.replaceAll('\\', '/')
          if (/^[a-zA-Z]\//.test(target)) target = `${target[0]}:/${target.slice(2)}`
          const path = target.toLocaleLowerCase().startsWith(`${root.toLocaleLowerCase()}/`)
            ? target.slice(root.length + 1)
            : target
          const next = values.flatMap((value) => {
            if (!value || typeof value !== 'object') return []
            const diagnostic = value as { range?: { start?: { line?: number; character?: number } }; severity?: number; message?: string }
            if (!diagnostic.range?.start) return []
            return [{ path, line: (diagnostic.range.start.line ?? 0) + 1, column: (diagnostic.range.start.character ?? 0) + 1, severity: diagnostic.severity ?? 3, message: diagnostic.message ?? '' }]
          })
          setProblems((current) => [...current.filter((problem) => problem.path !== path), ...next])
          const monaco = monacoRef.current
          const model = monaco?.editor.getModel(monaco.Uri.parse(uri))
          if (monaco && model) {
            monaco.editor.setModelMarkers(model, 'flux-code-lsp', next.map((problem) => ({
              message: problem.message,
              severity: problem.severity === 1 ? monaco.MarkerSeverity.Error : problem.severity === 2 ? monaco.MarkerSeverity.Warning : problem.severity === 3 ? monaco.MarkerSeverity.Info : monaco.MarkerSeverity.Hint,
              startLineNumber: problem.line,
              endLineNumber: problem.line,
              startColumn: problem.column,
              endColumn: problem.column + 1,
            })))
          }
        },
        setLanguageServerMessage,
      )
      if (monacoRef.current) client.registerProviders(monacoRef.current, finalLanguage)
      languageServerRef.current = client
      setLanguageServer(client)
      setLanguageServerLanguage(finalLanguage)
      setLanguageServerMessage('')
    } catch (cause) {
      setLanguageServerMessage(String(cause))
    }
  }

  const disconnectLanguageServer = async () => {
    await languageServerRef.current?.dispose()
    languageServerRef.current = null
    setLanguageServer(null)
    setLanguageServerLanguage('')
    setLanguageServerMessage('')
    setProblems([])
  }

  const formatActiveDocument = async () => {
    const document = activeDocument
    const editor = editorRef.current
    const client = languageServerRef.current
    if (!document || !editor || !client || languageServerLanguage !== languageForFile(document.path)) return
    try {
      const edits = await client.formatDocument(fileUri(`${projectPath}/${document.path}`))
      if (edits?.length) {
        const model = editor.getModel()
        if (!model) return
        editor.executeEdits('lsp-format', edits.map((edit) => ({
          range: {
            startLineNumber: edit.range.start.line + 1,
            startColumn: edit.range.start.character + 1,
            endLineNumber: edit.range.end.line + 1,
            endColumn: edit.range.end.character + 1,
          },
          text: edit.newText,
        })))
        editor.focus()
      }
    } catch (cause) {
      setLanguageServerMessage(String(cause))
    }
  }

  const goToDefinition = async () => {
    const document = activeDocument
    const editor = editorRef.current
    const client = languageServerRef.current
    const position = editor?.getPosition()
    if (!document || !editor || !client || !position) return
    try {
      const response = await client.goToDefinition(fileUri(`${projectPath}/${document.path}`), position)
      const locations = Array.isArray(response) ? response : response ? [response] : []
      const location = locations[0] as { uri?: string; targetUri?: string; range?: { start?: { line?: number; character?: number } }; targetRange?: { start?: { line?: number; character?: number } } } | undefined
      const uri = location?.uri ?? location?.targetUri
      const start = location?.range?.start ?? location?.targetRange?.start
      if (!uri || typeof start?.line !== 'number' || typeof start.character !== 'number') return
      const lineNumber = start.line + 1
      const column = start.character + 1
      const root = projectPath.replaceAll('\\', '/').replace(/\/$/, '')
      let target = uri.startsWith('file:///') ? decodeURIComponent(uri.slice('file:///'.length)) : uri
      target = target.replaceAll('\\', '/')
      if (/^[a-zA-Z]\//.test(target)) target = `${target[0]}:/${target.slice(2)}`
      if (!target.toLocaleLowerCase().startsWith(`${root.toLocaleLowerCase()}/`)) return
      const relativePath = target.slice(root.length + 1)
      await openFile({ path: relativePath, name: relativePath.split('/').at(-1) ?? relativePath, is_directory: false })
      window.setTimeout(() => editorRef.current?.revealPositionInCenter({ lineNumber, column }), 80)
    } catch (cause) {
      setLanguageServerMessage(String(cause))
    }
  }
  goToDefinitionRef.current = goToDefinition
  formatActiveDocumentRef.current = formatActiveDocument

  const navigateToDebugFrame = async (sourcePath: string, line: number) => {
    let target = sourcePath.replaceAll('\\', '/')
    if (target.startsWith('file:///')) target = target.slice('file:///'.length)
    try { target = decodeURIComponent(target) } catch { /* Keep the adapter path if it is not URI encoded. */ }
    const root = projectPath.replaceAll('\\', '/').replace(/\/$/, '')
    if (!target.toLocaleLowerCase().startsWith(`${root.toLocaleLowerCase()}/`)) return
    const relativePath = target.slice(root.length + 1)
    await openFile({ path: relativePath, name: relativePath.split('/').at(-1) ?? relativePath, is_directory: false })
    window.setTimeout(() => editorRef.current?.revealPositionInCenter({ lineNumber: line, column: 1 }), 80)
  }

  useEffect(() => () => {
    void languageServerRef.current?.dispose()
    languageServerRef.current = null
    setLanguageServer(null)
    setLanguageServerLanguage('')
    setProblems([])
  }, [projectPath])

  useEffect(() => {
    const languageId = activeDocument ? languageForFile(activeDocument.path) : ''
    if (!languageServer || languageServerLanguage !== languageId || !activeDocument) return
    languageServer.syncDocument(fileUri(`${projectPath}/${activeDocument.path}`), languageServerLanguage, activeDocument.content)
  }, [languageServer, languageServerLanguage, activeDocument?.path, activeDocument?.content, projectPath])

  useEffect(() => {
    const model = editorRef.current?.getModel()
    if (!model || !activePath) return
    const prior = breakpointDecorationIds.current[activePath] ?? []
    breakpointDecorationIds.current[activePath] = model.deltaDecorations(prior, (breakpoints[activePath] ?? []).map((line) => ({
      range: { startLineNumber: line, startColumn: 1, endLineNumber: line, endColumn: 1 },
      options: { glyphMarginClassName: 'debug-breakpoint-glyph', stickiness: 1 },
    })))
  }, [breakpoints, activePath, activeDocument?.path])

  breakpointHandlerRef.current = (event, monaco) => {
    if (event.target.type !== monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN) return
    const line = event.target.position?.lineNumber
    if (!line || !activePath) return
    setBreakpoints((current) => {
      const lines = new Set(current[activePath] ?? [])
      if (lines.has(line)) lines.delete(line)
      else lines.add(line)
      return { ...current, [activePath]: [...lines].sort((left, right) => left - right) }
    })
  }

  const refreshGit = async () => {
    if (!projectPath) return
    setGitLoading(true)
    try {
      const changes = await invoke<GitChange[]>('git_status', { projectPath })
      setGitChanges(changes)
      setGitError('')
      if (selectedGitPath && !changes.some((change) => change.path === selectedGitPath)) {
        setSelectedGitPath('')
        setGitDiff('')
      }
    } catch (cause) {
      setGitError(String(cause))
    } finally {
      setGitLoading(false)
    }
  }

  const selectGitChange = async (change: GitChange) => {
    setSelectedGitPath(change.path)
    try {
      setGitDiff(await invoke<string>('git_diff', { projectPath, relativePath: change.path }))
      setGitError('')
    } catch (cause) {
      setGitDiff('')
      setGitError(String(cause))
    }
  }

  const stageGitChange = async (change: GitChange) => {
    try {
      const shouldUnstage = change.indexStatus !== ' ' && change.worktreeStatus === ' ' && !change.untracked
      await invoke('git_stage', { projectPath, relativePath: change.path, staged: shouldUnstage })
      await refreshGit()
      const updated = gitChanges.find((candidate) => candidate.path === change.path)
      if (updated) await selectGitChange(updated)
    } catch (cause) {
      setGitError(String(cause))
    }
  }

  const commitGitChanges = async () => {
    try {
      await invoke('git_commit', { projectPath, message: commitMessage })
      setCommitMessage('')
      await refreshGit()
      setGitDiff('')
      setSelectedGitPath('')
    } catch (cause) {
      setGitError(String(cause))
    }
  }

  useEffect(() => {
    if (sidebarView === 'git') void refreshGit()
  }, [sidebarView, projectPath])

  const loadFolder = async (relativePath: string) => {
    if (!projectPath) return
    setLoadingPath(relativePath)
    try {
      const entries = await invoke<WorkspaceEntry[]>('list_workspace_directory', {
        projectPath,
        relativePath: relativePath || null,
      })
      setEntriesByFolder((current) => ({
        ...current,
        [relativePath]: entries,
      }))
      setError('')
    } catch (cause) {
      setError(`${t.loadError}: ${String(cause)}`)
    } finally {
      setLoadingPath(null)
    }
  }

  useEffect(() => {
    setEntriesByFolder({})
    setExpanded(new Set(['']))
    setDocuments([])
    setActivePath('')
    setFocusedFolder('')
    setError('')
    setRestoredProjectPath('')
    setSearchPage(null)
    if (!projectPath) return
    void Promise.all([
      loadFolder(''),
      (async () => {
        let state: { paths?: string[]; active?: string } = {}
        try {
          state = JSON.parse(localStorage.getItem(`${workspaceStateKey}${projectPath}`) ?? '{}') as typeof state
        } catch {
          state = {}
        }
        const paths = Array.isArray(state.paths) ? state.paths : []
        const restored = await Promise.all(paths.map(async (path) => {
          try {
            const content = await invoke<string>('read_workspace_file', { projectPath, relativePath: path })
            return { path, content, savedContent: content }
          } catch {
            return null
          }
        }))
        const documents = restored.filter((document): document is OpenDocument => document !== null)
        setDocuments(documents)
        setActivePath(documents.some((document) => document.path === state.active) ? state.active ?? '' : documents[0]?.path ?? '')
      })(),
    ]).finally(() => setRestoredProjectPath(projectPath))
  }, [projectPath])

  useEffect(() => {
    if (!projectPath || restoredProjectPath !== projectPath) return
    localStorage.setItem(`${workspaceStateKey}${projectPath}`, JSON.stringify({
      paths: documents.map((document) => document.path),
      active: activePath,
    }))
  }, [documents, activePath, projectPath, restoredProjectPath])

  const openFile = async (entry: WorkspaceEntry) => {
    const existing = documents.find((document) => document.path === entry.path)
    if (existing) {
      setActivePath(entry.path)
      return
    }
    setLoadingPath(entry.path)
    try {
      const content = await invoke<string>('read_workspace_file', { projectPath, relativePath: entry.path })
      setDocuments((current) => [...current, { path: entry.path, content, savedContent: content }])
      setActivePath(entry.path)
      setError('')
    } catch (cause) {
      setError(String(cause))
    } finally {
      setLoadingPath(null)
    }
  }

  const toggleFolder = async (path: string) => {
    if (expanded.has(path)) {
      setExpanded((current) => {
        const next = new Set(current)
        next.delete(path)
        return next
      })
      return
    }
    setExpanded((current) => new Set(current).add(path))
    if (!entriesByFolder[path]) await loadFolder(path)
  }

  const updateActiveContent = (content: string | undefined) => {
    if (content === undefined) return
    setDocuments((current) => current.map((document) => document.path === activePath ? { ...document, content } : document))
  }

  const saveDocument = async (document = activeDocument) => {
    if (!document || !projectPath) return
    try {
      await invoke('write_workspace_file', { projectPath, relativePath: document.path, content: document.content })
      setDocuments((current) => current.map((item) => item.path === document.path ? { ...item, savedContent: item.content } : item))
      if (languageServer && languageServerLanguage === languageForFile(document.path)) {
        languageServer.saveDocument(fileUri([projectPath, document.path].join(String.fromCharCode(47))))
      }
      setError('')
    } catch (cause) {
      setError(String(cause))
    }
  }

  const closeDocument = (path: string) => {
    const document = documents.find((item) => item.path === path)
    if (document && document.content !== document.savedContent && !window.confirm(t.discardChanges)) return
    setDocuments((current) => current.filter((item) => item.path !== path))
    if (activePath === path) setActivePath(documents.find((item) => item.path !== path)?.path ?? '')
  }

  const refreshVisibleFolders = async () => {
    await Promise.all([...expanded].map((path) => loadFolder(path)))
  }

  const createEntry = async (isDirectory: boolean) => {
    if (!projectPath) return
    const name = window.prompt(isDirectory ? t.newFolder : t.fileName)?.trim()
    if (!name) return
    const path = [focusedFolder, name].filter(Boolean).join('/')
    try {
      await invoke('create_workspace_entry', { projectPath, relativePath: path, isDirectory })
      await loadFolder(focusedFolder)
      setError('')
      if (!isDirectory) {
        const content = await invoke<string>('read_workspace_file', { projectPath, relativePath: path })
        setDocuments((current) => [...current.filter((item) => item.path !== path), { path, content, savedContent: content }])
        setActivePath(path)
      }
    } catch (cause) {
      setError(String(cause))
    }
  }

  const renameEntry = async (entry: WorkspaceEntry) => {
    const newName = window.prompt(t.rename, entry.name)?.trim()
    if (!newName || newName === entry.name) return
    try {
      const renamedPath = await invoke<string>('rename_workspace_entry', { projectPath, relativePath: entry.path, newName })
      const prefix = `${entry.path}/`
      setDocuments((current) => current.map((document) => document.path === entry.path || (entry.is_directory && document.path.startsWith(prefix))
        ? { ...document, path: document.path === entry.path ? renamedPath : `${renamedPath}/${document.path.slice(prefix.length)}` }
        : document))
      if (activePath === entry.path || (entry.is_directory && activePath.startsWith(prefix))) {
        setActivePath(activePath === entry.path ? renamedPath : `${renamedPath}/${activePath.slice(prefix.length)}`)
      }
      setEntriesByFolder({})
      setExpanded(new Set(['']))
      await loadFolder('')
    } catch (cause) {
      setError(String(cause))
    }
  }

  const deleteEntry = async (entry: WorkspaceEntry) => {
    if (!window.confirm(`${t.deleteConfirm}\n${entry.path}`)) return
    try {
      await invoke('delete_workspace_entry', { projectPath, relativePath: entry.path })
      const prefix = `${entry.path}/`
      setDocuments((current) => current.filter((document) => document.path !== entry.path && !(entry.is_directory && document.path.startsWith(prefix))))
      if (activePath === entry.path || (entry.is_directory && activePath.startsWith(prefix))) setActivePath('')
      setExpanded((current) => new Set([...current].filter((folder) => folder !== entry.path && !folder.startsWith(prefix))))
      const parent = entry.path.includes('/') ? entry.path.slice(0, entry.path.lastIndexOf('/')) : ''
      await loadFolder(parent)
    } catch (cause) {
      setError(String(cause))
    }
  }

  const runSearch = async (offset = 0) => {
    if (!projectPath || !searchQuery.trim()) return
    setSearching(true)
    setSearchOffset(offset)
    try {
      setSearchPage(await invoke<SearchPage>('search_workspace', { projectPath, query: searchQuery, offset, includeGenerated }))
      setError('')
    } catch (cause) {
      setError(String(cause))
    } finally {
      setSearching(false)
    }
  }

  const openSearchMatch = async (match: SearchMatch) => {
    if (!documents.some((document) => document.path === match.path)) {
      try {
        const content = await invoke<string>('read_workspace_file', { projectPath, relativePath: match.path })
        setDocuments((current) => [...current, { path: match.path, content, savedContent: content }])
      } catch (cause) {
        setError(String(cause))
        return
      }
    }
    setActivePath(match.path)
    if (match.line && editorRef.current) {
      window.setTimeout(() => {
        editorRef.current?.revealLineInCenter(match.line ?? 1)
        editorRef.current?.setPosition({ lineNumber: match.line ?? 1, column: 1 })
        editorRef.current?.focus()
      }, 80)
    }
  }

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 's' && activeDocument) {
        event.preventDefault()
        void saveDocument(activeDocument)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [activeDocument, projectPath])

  const tree = useMemo(() => {
    const renderFolder = (folderPath: string, depth: number): ReactNode => (entriesByFolder[folderPath] ?? []).map((entry) => (
      <div key={entry.path}>
        <button
          type="button"
          className={`explorer-entry ${entry.is_directory ? 'directory' : 'file'} ${activePath === entry.path ? 'active' : ''}`}
          style={{ paddingLeft: `${12 + depth * 14}px` }}
          onClick={() => {
            if (entry.is_directory) {
              setFocusedFolder(entry.path)
              void toggleFolder(entry.path)
            } else {
              void openFile(entry)
            }
          }}
          title={entry.path}
        >
          <span className={`tree-chevron ${entry.is_directory ? expanded.has(entry.path) ? 'expanded' : 'collapsed' : ''}`} />
          <span className={`tree-file-icon ${entry.is_directory ? 'folder-icon' : ''}`}>{entry.is_directory ? null : fileIcon(entry.name)}</span>
          <span className="explorer-entry-name">{entry.name}</span>
          {documents.some((document) => document.path === entry.path && document.content !== document.savedContent) ? <span className="dirty-dot" /> : null}
          <span className="explorer-entry-actions">
            <button type="button" className="explorer-rename-icon" title={t.rename} aria-label={`${t.rename} ${entry.name}`} onClick={(event) => { event.stopPropagation(); void renameEntry(entry) }} />
            <button type="button" className="explorer-delete-icon" title={t.delete} aria-label={`${t.delete} ${entry.name}`} onClick={(event) => { event.stopPropagation(); void deleteEntry(entry) }} />
          </span>
        </button>
        {entry.is_directory && expanded.has(entry.path) ? renderFolder(entry.path, depth + 1) : null}
      </div>
    ))
    return renderFolder('', 0)
  }, [entriesByFolder, expanded, activePath, documents, focusedFolder, language])

  if (!projectPath) return <div className="ide-empty"><div className="ide-empty-mark" /><h2>{t.noFolder}</h2><button type="button" className="manager-button primary" onClick={onChooseProject}>{t.openFolder}</button></div>

  return (
    <div className="ide-shell">
      <aside className="ide-explorer">
        <div className="ide-section-heading"><span>{sidebarView === 'explorer' ? t.explorer : sidebarView === 'git' ? t.git : t.problems}</span><div className="ide-toolbar-actions">
          <button type="button" className={`sidebar-git-icon ${sidebarView === 'git' ? 'active' : ''}`} title={t.git} aria-label={t.git} onClick={() => setSidebarView((current) => current === 'git' ? 'explorer' : 'git')} />
          <button type="button" className={`sidebar-problems-icon ${sidebarView === 'problems' ? 'active' : ''}`} title={problemCountLabel(t.problemCount, problems.length)} aria-label={problemCountLabel(t.problemCount, problems.length)} onClick={() => setSidebarView((current) => current === 'problems' ? 'explorer' : 'problems')}>{problems.length || ''}</button>
          {sidebarView === 'explorer' ? <>
          <button type="button" className="new-file-icon" title={t.newFile} aria-label={t.newFile} onClick={() => void createEntry(false)} />
          <button type="button" className="new-folder-icon" title={t.newFolder} aria-label={t.newFolder} onClick={() => void createEntry(true)} />
          <button type="button" className="refresh-icon" title={t.refresh} aria-label={t.refresh} onClick={() => void refreshVisibleFolders()} />
          </> : <button type="button" className="refresh-icon" title={t.refreshGit} aria-label={t.refreshGit} onClick={() => void refreshGit()} />}
        </div></div>
        {sidebarView === 'explorer' ? <>
        <button type="button" className="explorer-root" onClick={() => { setFocusedFolder(''); setExpanded((current) => new Set(current).add('')) }}><span className="root-chevron" /> <span>{rootName.toUpperCase()}</span></button>
        <div className="ide-search-box">
          <input type="search" value={searchQuery} onChange={(event) => setSearchQuery(event.target.value)} placeholder={t.searchPlaceholder} aria-label={t.search} onKeyDown={(event) => { if (event.key === 'Enter') void runSearch(0) }} />
          <button type="button" className="ide-search-action" title={t.search} aria-label={t.search} disabled={searching || !searchQuery.trim()} onClick={() => void runSearch(0)} />
        </div>
        <label className="ide-generated-toggle"><input type="checkbox" checked={includeGenerated} onChange={(event) => setIncludeGenerated(event.target.checked)} />{t.includeGenerated}</label>
        <div className="explorer-tree">{searchPage ? <div className="ide-search-results">
          <div className="ide-search-summary">{searchPage.total === 0 ? t.noSearchResults : searchCountLabel(t.searchCount, searchOffset + 1, Math.min(searchOffset + searchPage.page_size, searchPage.total), searchPage.total)}</div>
          {searchPage.matches.map((match, index) => <button type="button" className="ide-search-result" key={`${match.path}-${match.line ?? 'file'}-${index}`} onClick={() => void openSearchMatch(match)} title={match.path}>
            <strong>{searchResultLabel(match.path, match.line)}</strong><span>{match.preview}</span>
          </button>)}
          <div className="ide-search-pagination">
            <button type="button" className="ide-prev-page" disabled={searching || searchOffset === 0} title={t.previousPage} aria-label={t.previousPage} onClick={() => void runSearch(Math.max(0, searchOffset - searchPage.page_size))} />
            <button type="button" className="ide-next-page" disabled={searching || searchOffset + searchPage.page_size >= searchPage.total} title={t.nextPage} aria-label={t.nextPage} onClick={() => void runSearch(searchOffset + searchPage.page_size)} />
          </div>
        </div> : loadingPath === '' && !entriesByFolder[''] ? <div className="ide-tree-message">{t.loading}</div> : tree}</div>
        {error ? <div className="ide-error">{error}</div> : null}
        </> : sidebarView === 'git' ? <div className="git-sidebar-content">
          <div className="git-commit-form"><textarea value={commitMessage} onChange={(event) => setCommitMessage(event.target.value)} placeholder={t.commitPlaceholder} aria-label={t.commitPlaceholder} /><button type="button" className="manager-button primary" disabled={!commitMessage.trim() || !gitChanges.some((change) => change.indexStatus !== ' ' && change.indexStatus !== '?')} onClick={() => void commitGitChanges()}>{t.commit}</button></div>
          {gitLoading ? <div className="ide-tree-message">{t.loading}</div> : null}
          {gitChanges.map((change) => <div className={`git-change ${selectedGitPath === change.path ? 'active' : ''}`} key={change.path}>
            <button type="button" className="git-change-select" onClick={() => void selectGitChange(change)}><span className="git-change-status">{change.untracked ? t.untrackedStatus : change.indexStatus !== ' ' ? change.indexStatus : change.worktreeStatus}</span><span>{change.path}</span></button>
            <button type="button" className="git-stage-button" title={change.indexStatus !== ' ' && change.worktreeStatus === ' ' && !change.untracked ? t.unstage : t.stage} aria-label={`${change.indexStatus !== ' ' && change.worktreeStatus === ' ' && !change.untracked ? t.unstage : t.stage} ${change.path}`} onClick={() => void stageGitChange(change)}>{change.indexStatus !== ' ' && change.worktreeStatus === ' ' && !change.untracked ? t.unstageSymbol : t.stageSymbol}</button>
          </div>)}
          {!gitLoading && gitChanges.length === 0 && !gitError ? <div className="ide-tree-message">{t.noGitChanges}</div> : null}
          {gitError ? <div className="ide-error">{gitError}</div> : null}
          {selectedGitPath ? <pre className="git-diff-view">{gitDiff || selectedGitPath}</pre> : null}
        </div> : <div className="git-sidebar-content">
          {problems.length === 0 ? <div className="ide-tree-message">{t.noProblems}</div> : problems.map((problem, index) => <button type="button" className="problem-entry" key={[problem.path, problem.line, problem.column, index].join(String.fromCharCode(58))} onClick={() => {
            const path = problem.path.replaceAll('\\', '/')
            void openFile({ path, name: path.split('/').at(-1) ?? path, is_directory: false }).then(() => {
              window.setTimeout(() => editorRef.current?.revealPositionInCenter({ lineNumber: problem.line, column: problem.column }), 80)
            })
          }}><span className={`problem-severity severity-${problem.severity}`} /><span className="problem-message"><strong>{problem.message}</strong><small>{problemLocationLabel(t.problemLocation, problem)}</small></span></button>)}
          {languageServerMessage ? <div className="ide-error">{languageServerMessage}</div> : null}
        </div>}
      </aside>
      <div className="ide-workbench">
      <section className="ide-editor-area">
        <div className="ide-tabs">
          <button type="button" className={`ide-terminal-toggle ${terminalOpen ? 'active' : ''}`} title={t.terminal} aria-label={t.terminal} onClick={() => setTerminalOpen((current) => !current)} />
          <button type="button" className={`ide-lsp-toggle ${languageServer ? 'active' : ''}`} title={languageServer ? t.stopLanguageServer : t.connectLanguageServer} aria-label={languageServer ? t.stopLanguageServer : t.connectLanguageServer} onClick={() => void (languageServer ? disconnectLanguageServer() : connectLanguageServer())}>{t.languageServerShort}</button>
          <button type="button" className={`ide-debug-toggle ${debugOpen ? 'active' : ''}`} title={t.debug} aria-label={t.debug} onClick={() => setDebugOpen((current) => !current)}>{t.debugShort}</button>
          {documents.map((document) => <div className={`ide-tab ${document.path === activePath ? 'active' : ''}`} key={document.path}>
            <button type="button" className="ide-tab-select" onClick={() => setActivePath(document.path)} title={document.path}>
              <span className="tree-file-icon">{fileIcon(document.path.split('/').at(-1) ?? document.path)}</span>
              <span>{document.path.split('/').at(-1)}</span>
              {document.content !== document.savedContent ? <span className="dirty-dot" /> : null}
            </button>
            <button type="button" className="ide-tab-close" title={closeFileLabel(t.closeFile, document.path)} aria-label={closeFileLabel(t.closeFile, document.path)} onClick={() => closeDocument(document.path)} />
          </div>)}
        </div>
        {activeDocument ? <>
          <div className="ide-breadcrumb"><span>{rootName}</span><span className="ide-crumb-separator" /><span>{displayPath(activeDocument.path)}</span><span className="ide-save-status">{languageServerMessage || (activeDocument.content !== activeDocument.savedContent ? t.unsaved : t.saved)}</span>
            {languageServer && languageServerLanguage === languageForFile(activeDocument.path) ? <>
              <button type="button" className="ide-save-button" title={t.goToDefinitionTitle} onClick={() => void goToDefinition()}>{t.goToDefinition}</button>
              <button type="button" className="ide-save-button" title={t.formatTitle} onClick={() => void formatActiveDocument()}>{t.formatLabel}</button>
            </> : null}
            <button type="button" className="ide-save-button" onClick={() => void saveDocument()}>{t.save}</button>
          </div>
          <div className="ide-monaco-editor"><Editor
            height="100%"
            path={fileUri([projectPath, activeDocument.path].join(String.fromCharCode(47)))}
            language={languageForFile(activeDocument.path)}
            value={activeDocument.content}
            onChange={updateActiveContent}
            onMount={(instance, monaco) => { editorRef.current = instance; monacoRef.current = monaco; instance.onMouseDown((event) => breakpointHandlerRef.current(event, monaco)); if (languageServer && languageServerLanguage === languageForFile(activeDocument.path)) languageServer.registerProviders(monaco, languageServerLanguage); instance.addAction({ id: 'flux-lsp-go-to-definition', label: t.goToDefinition, keybindings: [monaco.KeyCode.F12], run: () => goToDefinitionRef.current() }); instance.addAction({ id: 'flux-lsp-format-document', label: t.formatDocument, keybindings: [monaco.KeyMod.Shift | monaco.KeyMod.Alt | monaco.KeyCode.KeyF], run: () => formatActiveDocumentRef.current() }) }}
            onValidate={(markers) => {
              const next = markers.map((marker) => ({ path: activePath, line: marker.startLineNumber, column: marker.startColumn, severity: marker.severity === 8 ? 1 : marker.severity === 4 ? 2 : marker.severity === 2 ? 3 : 4, message: marker.message }))
              setProblems((current) => [...current.filter((problem) => problem.path !== activePath), ...next])
            }}
            theme={editorTheme}
            options={editorOptions}
          /></div>
        </> : <div className="ide-welcome"><div className="ide-welcome-icon" /><p>{t.selectFile}</p><span>{t.saveShortcut}</span></div>}
      </section>
      <TerminalPanel projectPath={projectPath} language={language} visible={terminalOpen} onClose={() => setTerminalOpen(false)} />
      <DebugPanel projectPath={projectPath} language={language} visible={debugOpen} activeFile={activePath} breakpoints={breakpoints} onClose={() => setDebugOpen(false)} onNavigate={(path, line) => void navigateToDebugFrame(path, line)} />
      </div>
    </div>
  )
}
