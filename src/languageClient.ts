import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { Monaco } from '@monaco-editor/react'
import type { editor, languages, Position } from 'monaco-editor'

interface ServerEvent {
  sessionId: string
  message: string
}
interface ServerExitEvent { sessionId: string; detail: string }

interface LspPosition { line: number; character: number }
interface LspRange { start: LspPosition; end: LspPosition }
interface LspLocation { uri: string; range: LspRange }

export function fileUri(path: string) {
  const normalized = path.replaceAll('\\', '/')
  const segments = normalized.split('/')
  return `file:///${segments[0]}${segments.slice(1).map((segment) => `/${encodeURIComponent(segment)}`).join('')}`
}

export class LanguageClient {
  readonly sessionId = crypto.randomUUID()
  private readonly pending = new Map<number, { resolve: (value: unknown) => void; reject: (reason: Error) => void }>()
  private readonly versions = new Map<string, { version: number; text: string }>()
  private readonly disposables: { dispose: () => void }[] = []
  private readonly registeredLanguages = new Set<string>()
  private listeners: UnlistenFn[] = []
  private nextId = 0
  private stopped = false
  private readonly projectPath: string
  readonly languageId: string
  private readonly onDiagnostics: (uri: string, diagnostics: unknown[]) => void
  private readonly onLog: (message: string) => void

  private constructor(
    projectPath: string,
    languageId: string,
    onDiagnostics: (uri: string, diagnostics: unknown[]) => void,
    onLog: (message: string) => void,
  ) {
    this.projectPath = projectPath
    this.languageId = languageId
    this.onDiagnostics = onDiagnostics
    this.onLog = onLog
  }

  static async start(
    projectPath: string,
    languageId: string,
    command: string,
    args: string[],
    onDiagnostics: (uri: string, diagnostics: unknown[]) => void,
    onLog: (message: string) => void,
  ) {
    const client = new LanguageClient(projectPath, languageId, onDiagnostics, onLog)
    client.listeners.push(await listen<ServerEvent>('language-server-message', (event) => {
      if (event.payload.sessionId !== client.sessionId) return
      client.receive(event.payload.message)
    }))
    client.listeners.push(await listen<ServerExitEvent>('language-server-exit', (event) => {
      if (event.payload.sessionId === client.sessionId && !client.stopped) client.onLog(event.payload.detail || 'Language server exited')
    }))
    try {
      await invoke('start_language_server', {
        sessionId: client.sessionId,
        command,
        args,
        projectPath,
      })
      const rootUri = fileUri(projectPath)
      await client.request('initialize', {
        processId: null,
        clientInfo: { name: 'Flux Code', version: '0.1.0' },
        rootUri,
        workspaceFolders: [{ uri: rootUri, name: projectPath.split(/[\\/]/).filter(Boolean).at(-1) ?? 'workspace' }],
        capabilities: {
          general: { positionEncodings: ['utf-16'] },
          textDocument: {
            completion: { completionItem: { snippetSupport: false, documentationFormat: ['markdown', 'plaintext'] } },
            definition: { linkSupport: false },
            hover: { contentFormat: ['markdown', 'plaintext'] },
            documentFormatting: {},
            publishDiagnostics: { relatedInformation: true },
          },
          workspace: { workspaceFolders: true, configuration: true, applyEdit: true },
        },
      })
      client.notify('initialized', {})
      return client
    } catch (error) {
      await client.dispose()
      throw error
    }
  }

  private async send(message: Record<string, unknown>) {
    if (this.stopped) throw new Error('Language server is stopped')
    await invoke('send_language_server_message', {
      sessionId: this.sessionId,
      message: JSON.stringify(message),
    })
  }

  private async request(method: string, params: unknown) {
    const id = ++this.nextId
    const response = new Promise<unknown>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(id)
        reject(new Error(`Language server timed out: ${method}`))
      }, 15000)
      this.pending.set(id, {
        resolve: (value) => { window.clearTimeout(timer); resolve(value) },
        reject: (reason) => { window.clearTimeout(timer); reject(reason) },
      })
    })
    try {
      await this.send({ jsonrpc: '2.0', id, method, params })
    } catch (error) {
      this.pending.delete(id)
      throw error
    }
    return response
  }

  private notify(method: string, params: unknown) {
    void this.send({ jsonrpc: '2.0', method, params }).catch((error: unknown) => this.onLog(String(error)))
  }

  private receive(body: string) {
    let message: Record<string, unknown>
    try { message = JSON.parse(body) as Record<string, unknown> } catch { this.onLog('Language server sent invalid JSON'); return }
    if (message.method === 'textDocument/publishDiagnostics') {
      const params = message.params as { uri?: string; diagnostics?: unknown[] } | undefined
      if (params?.uri) this.onDiagnostics(params.uri, params.diagnostics ?? [])
    }
    if (typeof message.id === 'number' || typeof message.id === 'string') {
      const pending = typeof message.id === 'number' ? this.pending.get(message.id) : undefined
      if (pending) {
        this.pending.delete(message.id as number)
        if (message.error) pending.reject(new Error(JSON.stringify(message.error)))
        else pending.resolve(message.result)
      } else if (typeof message.method === 'string') {
        void this.replyToServerRequest(message)
      }
    }
    if (message.method === 'window/showMessage' || message.method === 'window/logMessage') {
      const params = message.params as { message?: string } | undefined
      if (params?.message) this.onLog(params.message)
    }
  }

  private async replyToServerRequest(message: Record<string, unknown>) {
    const method = String(message.method)
    const params = message.params as { items?: unknown[] } | undefined
    let result: unknown
    if (method === 'workspace/workspaceFolders') {
      result = [{ uri: fileUri(this.projectPath), name: this.projectPath.split(/[\\/]/).filter(Boolean).at(-1) ?? 'workspace' }]
    } else if (method === 'workspace/configuration') {
      result = (params?.items ?? []).map(() => null)
    } else if (method === 'window/workDoneProgress/create' || method === 'client/registerCapability' || method === 'client/unregisterCapability') {
      result = null
    } else {
      await this.send({ jsonrpc: '2.0', id: message.id, error: { code: -32601, message: `Unsupported client request: ${method}` } }).catch(() => undefined)
      return
    }
    await this.send({ jsonrpc: '2.0', id: message.id, result }).catch((error: unknown) => this.onLog(String(error)))
  }

  syncDocument(uri: string, languageId: string, text: string) {
    const prior = this.versions.get(uri)
    if (!prior) {
      this.versions.set(uri, { version: 1, text })
      this.notify('textDocument/didOpen', { textDocument: { uri, languageId, version: 1, text } })
    } else if (prior.text !== text) {
      const version = prior.version + 1
      this.versions.set(uri, { version, text })
      this.notify('textDocument/didChange', { textDocument: { uri, version }, contentChanges: [{ text }] })
    }
  }

  saveDocument(uri: string) { this.notify('textDocument/didSave', { textDocument: { uri } }) }

  registerProviders(monaco: Monaco, languageId: string) {
    if (this.registeredLanguages.has(languageId)) return
    this.registeredLanguages.add(languageId)
    const request = (method: string, model: editor.ITextModel, position?: Position) => {
      const params = {
        textDocument: { uri: model.uri.toString() },
        ...(position ? { position: { line: position.lineNumber - 1, character: position.column - 1 } } : {}),
      }
      return this.request(method, params)
    }
    const completionProvider: languages.CompletionItemProvider = {
      provideCompletionItems: async (model, position) => {
        const response = await request('textDocument/completion', model, position) as { items?: Record<string, unknown>[] } | Record<string, unknown>[] | null
        const items = Array.isArray(response) ? response : response?.items ?? []
        return { suggestions: items.filter((item) => typeof item.label === 'string').map((item) => ({
          label: item.label as string,
          kind: (item.kind as number | undefined) ?? monaco.languages.CompletionItemKind.Text,
          insertText: (item.insertText as string | undefined) ?? item.label as string,
          detail: item.detail as string | undefined,
          documentation: item.documentation as string | undefined,
          range: new monaco.Range(position.lineNumber, model.getWordUntilPosition(position).startColumn, position.lineNumber, position.column),
        })) }
      },
    }
    this.disposables.push(monaco.languages.registerCompletionItemProvider(languageId, completionProvider))
    const definitionProvider: languages.DefinitionProvider = {
      provideDefinition: async (model, position) => {
        const response = await request('textDocument/definition', model, position)
        const locations = Array.isArray(response) ? response : response ? [response] : []
        return locations.flatMap((item) => {
          const location = item as LspLocation
          if (!location.uri || !location.range?.start || !location.range?.end) return []
          return [{ uri: monaco.Uri.parse(location.uri), range: new monaco.Range(location.range.start.line + 1, location.range.start.character + 1, location.range.end.line + 1, location.range.end.character + 1) }]
        })
      },
    }
    this.disposables.push(monaco.languages.registerDefinitionProvider(languageId, definitionProvider))
    const hoverProvider: languages.HoverProvider = {
      provideHover: async (model, position) => {
        const response = await request('textDocument/hover', model, position) as { contents?: unknown } | null
        if (!response?.contents) return null
        const contents = Array.isArray(response.contents) ? response.contents : [response.contents]
        return { contents: contents.map((item) => typeof item === 'string' ? { value: item } : typeof item === 'object' && item !== null && 'value' in item ? { value: String(item.value) } : { value: String(item) }) }
      },
    }
    this.disposables.push(monaco.languages.registerHoverProvider(languageId, hoverProvider))
    const formattingProvider: languages.DocumentFormattingEditProvider = {
      provideDocumentFormattingEdits: async (model) => {
        const edits = await request('textDocument/formatting', model) as { range: LspRange; newText: string }[] | null
        return (edits ?? []).map((edit) => ({
          range: new monaco.Range(edit.range.start.line + 1, edit.range.start.character + 1, edit.range.end.line + 1, edit.range.end.character + 1),
          text: edit.newText,
        }))
      },
    }
    this.disposables.push(monaco.languages.registerDocumentFormattingEditProvider(languageId, formattingProvider))
  }

  async dispose() {
    if (this.stopped) return
    for (const uri of this.versions.keys()) {
      await this.send({ jsonrpc: '2.0', method: 'textDocument/didClose', params: { textDocument: { uri } } }).catch(() => undefined)
    }
    this.stopped = true
    this.listeners.forEach((unlisten) => unlisten())
    for (const pending of this.pending.values()) pending.reject(new Error('Language server stopped'))
    this.pending.clear()
    this.disposables.forEach((disposable) => disposable.dispose())
    await invoke('stop_language_server', { sessionId: this.sessionId }).catch(() => undefined)
  }
}
