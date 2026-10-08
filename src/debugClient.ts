import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

interface ProtocolEvent { sessionId: string; message: string }
interface ProtocolExit { sessionId: string; detail: string }
export interface DebugEvent { event: string; body?: Record<string, unknown> }
export interface DebugFrame { id: number; name: string; line: number; column: number; source?: { name?: string; path?: string } }

export class DebugAdapterClient {
  readonly sessionId = crypto.randomUUID()
  private nextSequence = 0
  private pending = new Map<number, { resolve: (value: Record<string, unknown>) => void; reject: (reason: Error) => void }>()
  private listeners: UnlistenFn[] = []
  private initializedResolve?: () => void
  private initialized = new Promise<void>((resolve) => { this.initializedResolve = resolve })
  private supportsConfigurationDone = false
  private closed = false
  private readonly onEvent: (event: DebugEvent) => void
  private readonly onLog: (text: string) => void
  private readonly onFrames: (frames: DebugFrame[]) => void

  private constructor(onEvent: (event: DebugEvent) => void, onLog: (text: string) => void, onFrames: (frames: DebugFrame[]) => void) {
    this.onEvent = onEvent
    this.onLog = onLog
    this.onFrames = onFrames
  }

  static async start(projectPath: string, command: string, args: string[], locale: string, onEvent: (event: DebugEvent) => void, onLog: (text: string) => void, onFrames: (frames: DebugFrame[]) => void) {
    const client = new DebugAdapterClient(onEvent, onLog, onFrames)
    client.listeners.push(await listen<ProtocolEvent>('language-server-message', (event) => {
      if (event.payload.sessionId === client.sessionId) void client.receive(event.payload.message)
    }))
    client.listeners.push(await listen<ProtocolExit>('language-server-exit', (event) => {
      if (event.payload.sessionId === client.sessionId && !client.closed) client.onLog(event.payload.detail || 'Debug adapter exited')
    }))
    try {
      await invoke('start_language_server', { sessionId: client.sessionId, command, args, projectPath })
      const response = await client.request('initialize', {
        clientID: 'flux-code', clientName: 'Flux Code', adapterID: 'configured-adapter', locale,
        linesStartAt1: true, columnsStartAt1: true, pathFormat: 'path', supportsVariableType: true,
        supportsRunInTerminalRequest: false, supportsConfigurationDoneRequest: true,
      })
      client.supportsConfigurationDone = Boolean(response.body && (response.body as Record<string, unknown>).supportsConfigurationDoneRequest)
      return client
    } catch (error) {
      await client.dispose()
      throw error
    }
  }

  private async send(message: Record<string, unknown>) {
    if (this.closed) throw new Error('Debug adapter is stopped')
    await invoke('send_language_server_message', { sessionId: this.sessionId, message: JSON.stringify(message) })
  }

  private request(command: string, args: Record<string, unknown> = {}) {
    const seq = ++this.nextSequence
    const response = new Promise<Record<string, unknown>>((resolve, reject) => {
      const timer = window.setTimeout(() => { this.pending.delete(seq); reject(new Error(`Debug adapter timed out: ${command}`)) }, 30000)
      this.pending.set(seq, {
        resolve: (value) => { window.clearTimeout(timer); resolve(value) },
        reject: (error) => { window.clearTimeout(timer); reject(error) },
      })
    })
    void this.send({ seq, type: 'request', command, arguments: args }).catch((error: unknown) => {
      const pending = this.pending.get(seq)
      if (pending) { this.pending.delete(seq); pending.reject(error instanceof Error ? error : new Error(String(error))) }
    })
    return response
  }

  private async receive(raw: string) {
    let message: Record<string, unknown>
    try { message = JSON.parse(raw) as Record<string, unknown> } catch { this.onLog('Debug adapter sent invalid JSON'); return }
    if (message.type === 'response' && typeof message.request_seq === 'number') {
      const pending = this.pending.get(message.request_seq)
      if (pending) {
        this.pending.delete(message.request_seq)
        if (message.success === false) pending.reject(new Error(String(message.message ?? message.command ?? 'Debug request failed')))
        else pending.resolve(message)
      }
    } else if (message.type === 'event' && typeof message.event === 'string') {
      const event = { event: message.event, body: message.body as Record<string, unknown> | undefined }
      this.onEvent(event)
      if (event.event === 'initialized') this.initializedResolve?.()
      if (event.event === 'output' && typeof event.body?.output === 'string') this.onLog(event.body.output)
      if (event.event === 'stopped') await this.loadStack(Number(event.body?.threadId ?? 0))
      if (event.event === 'terminated' || event.event === 'exited') this.onFrames([])
    } else if (message.type === 'request' && typeof message.command === 'string') {
      await this.send({ seq: ++this.nextSequence, type: 'response', request_seq: message.seq, command: message.command, success: false, message: 'This debugger request is not supported by Flux Code yet.' })
    }
  }

  async launch(configuration: Record<string, unknown>, breakpoints: Record<string, number[]>) {
    await Promise.race([this.initialized, new Promise<never>((_, reject) => window.setTimeout(() => reject(new Error('Debug adapter did not send initialized event')), 20000))])
    for (const [path, lines] of Object.entries(breakpoints)) await this.setBreakpoints(path, lines)
    await this.request('setExceptionBreakpoints', { filters: [] })
    if (this.supportsConfigurationDone) await this.request('configurationDone')
    const request = configuration.request === 'attach' ? 'attach' : 'launch'
    await this.request(request, configuration)
  }

  async setBreakpoints(path: string, lines: number[]) {
    return this.request('setBreakpoints', {
      source: { name: path.split(/[\\/]/).at(-1), path },
      sourceModified: false,
      breakpoints: lines.map((line) => ({ line })),
    })
  }

  async continueExecution(threadId: number) { return this.request('continue', { threadId }) }
  async next(threadId: number) { return this.request('next', { threadId }) }
  async stepIn(threadId: number) { return this.request('stepIn', { threadId }) }
  async stepOut(threadId: number) { return this.request('stepOut', { threadId }) }
  async pause(threadId: number) { return this.request('pause', { threadId }) }

  private async loadStack(threadId: number) {
    try {
      const threads = await this.request('threads')
      const list = (threads.body as { threads?: { id: number }[] } | undefined)?.threads ?? []
      const actualThreadId = list.some((thread) => thread.id === threadId) ? threadId : list[0]?.id
      if (actualThreadId === undefined) return this.onFrames([])
      const response = await this.request('stackTrace', { threadId: actualThreadId, startFrame: 0, levels: 50 })
      const frames = (response.body as { stackFrames?: DebugFrame[] } | undefined)?.stackFrames ?? []
      this.onFrames(frames)
    } catch (error) { this.onLog(String(error)) }
  }

  async disconnect() {
    if (!this.closed) await this.request('disconnect', { restart: false, terminateDebuggee: true }).catch((error: unknown) => this.onLog(String(error)))
    await this.dispose()
  }

  async dispose() {
    if (this.closed) return
    this.closed = true
    this.listeners.forEach((unlisten) => unlisten())
    for (const pending of this.pending.values()) pending.reject(new Error('Debug adapter stopped'))
    this.pending.clear()
    await invoke('stop_language_server', { sessionId: this.sessionId }).catch(() => undefined)
  }
}
