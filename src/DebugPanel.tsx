import { useEffect, useState } from 'react'
import { DebugAdapterClient, type DebugEvent, type DebugFrame } from './debugClient'

interface DebugPanelProps {
  projectPath: string
  language: 'en' | 'ru'
  visible: boolean
  activeFile: string
  breakpoints: Record<string, number[]>
  onClose: () => void
  onNavigate: (path: string, line: number) => void
}

const labels = {
  en: { title: 'RUN AND DEBUG', start: 'Start debugging', stop: 'Stop', command: 'Debug adapter command', args: 'Adapter arguments as a JSON array', launch: 'Launch configuration as JSON', launchExample: '{"request":"launch","program":"${file}","cwd":"${workspaceFolder}"}', continue: 'Continue', next: 'Step over', stepIn: 'Step into', stepOut: 'Step out', pause: 'Pause', close: 'Close debug panel', noFrames: 'Call stack will appear when execution pauses', disconnected: 'Not connected', stackLocation: '{{source}} · {{line}}' },
  ru: { title: 'ЗАПУСК И ОТЛАДКА', start: 'Начать отладку', stop: 'Остановить', command: 'Команда debug adapter', args: 'Аргументы адаптера в JSON-массиве', launch: 'Конфигурация запуска в JSON', launchExample: '{"request":"launch","program":"${file}","cwd":"${workspaceFolder}"}', continue: 'Продолжить', next: 'Шаг через', stepIn: 'Шаг внутрь', stepOut: 'Шаг наружу', pause: 'Пауза', close: 'Закрыть отладчик', noFrames: 'Стек вызовов появится после остановки', disconnected: 'Не подключён', stackLocation: '{{source}} · строка {{line}}' },
}

function replaceVariables(value: unknown, variables: Record<string, string>): unknown {
  if (typeof value === 'string') return value.replace(/\$\{(workspaceFolder|file)\}/g, (_match, key: string) => variables[key] ?? '')
  if (Array.isArray(value)) return value.map((entry) => replaceVariables(entry, variables))
  if (typeof value === 'object' && value !== null) return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, replaceVariables(entry, variables)]))
  return value
}

function nativePath(root: string, relative: string) {
  return `${root.replace(/[\\/]$/, '')}\\${relative.replaceAll('/', '\\')}`
}

function stackLocation(template: string, frame: DebugFrame) {
  return template
    .replace(/\{\{source\}\}/, frame.source?.name ?? frame.source?.path ?? '')
    .replace(/\{\{line\}\}/, String(frame.line))
}

function debugLog(logs: string[], error: string) {
  return `${logs.join('')}${error ? String.fromCharCode(10) + error : ''}`
}

export function DebugPanel({ projectPath, language, visible, activeFile, breakpoints, onClose, onNavigate }: DebugPanelProps) {
  const t = labels[language]
  const [client, setClient] = useState<DebugAdapterClient | null>(null)
  const [threadId, setThreadId] = useState(0)
  const [status, setStatus] = useState(t.disconnected)
  const [logs, setLogs] = useState<string[]>([])
  const [frames, setFrames] = useState<DebugFrame[]>([])
  const [debugActive, setDebugActive] = useState(false)
  const [error, setError] = useState('')

  const start = async () => {
    const key = `flux-code-debug:${projectPath}`
    let saved: { command?: string; args?: string[]; launch?: string } = {}
    try { saved = JSON.parse(localStorage.getItem(key) ?? '{}') as typeof saved } catch { saved = {} }
    const command = window.prompt(t.command, saved.command ?? '')?.trim()
    if (!command) return
    const rawArgs = window.prompt(t.args, JSON.stringify(saved.args ?? []))
    if (rawArgs === null) return
    const rawLaunch = window.prompt(t.launch, saved.launch ?? t.launchExample)
    if (rawLaunch === null) return
    try {
      const args: unknown = JSON.parse(rawArgs)
      const launch: unknown = JSON.parse(rawLaunch)
      if (!Array.isArray(args) || args.some((item) => typeof item !== 'string') || typeof launch !== 'object' || launch === null || Array.isArray(launch)) throw new Error(t.launch)
      localStorage.setItem(key, JSON.stringify({ command, args, launch: rawLaunch }))
      const breakpointsByPath = Object.fromEntries(Object.entries(breakpoints).map(([path, lines]) => [nativePath(projectPath, path), lines]))
      const adapter = await DebugAdapterClient.start(
        projectPath,
        command,
        args as string[],
        language === 'ru' ? 'ru-RU' : 'en-US',
        (event: DebugEvent) => {
          setStatus(event.event)
          if (event.event === 'stopped') setThreadId(Number(event.body?.threadId ?? 0))
          if (event.event === 'continued') setStatus(t.continue)
          if (event.event === 'terminated' || event.event === 'exited') { setThreadId(0); setFrames([]) }
        },
        (text) => setLogs((current) => [...current.slice(-299), text]),
        setFrames,
      )
      setClient(adapter)
      setError('')
      setStatus(t.start)
      const file = activeFile ? nativePath(projectPath, activeFile) : ''
      const config = replaceVariables(launch, { workspaceFolder: projectPath, file }) as Record<string, unknown>
      await adapter.launch(config, breakpointsByPath)
      setDebugActive(true)
      setStatus(t.continue)
    } catch (cause) {
      setError(String(cause))
      setStatus(t.disconnected)
    }
  }

  const stop = async () => {
    await client?.disconnect()
    setClient(null)
    setThreadId(0)
    setFrames([])
    setDebugActive(false)
    setStatus(t.disconnected)
  }

  useEffect(() => {
    if (!client || !debugActive) return
    const sources = Object.entries(breakpoints)
    for (const [path, lines] of sources) void client.setBreakpoints(nativePath(projectPath, path), lines).catch((cause) => setError(String(cause)))
  }, [client, debugActive, breakpoints, projectPath])

  useEffect(() => () => { void client?.dispose() }, [client])

  const run = (operation: (id: number) => Promise<unknown>) => {
    if (!threadId) return
    void operation(threadId).catch((cause) => setError(String(cause)))
  }

  return (
    <section className={`ide-debug-panel ${visible ? 'visible' : 'hidden'}`} aria-hidden={!visible}>
      <div className="ide-debug-heading"><span>{t.title}</span><span className="ide-debug-status">{error || status}</span><button type="button" className="ide-terminal-close" title={t.close} aria-label={t.close} onClick={onClose} /></div>
      <div className="ide-debug-toolbar">
        {client ? <>
          <button type="button" title={t.continue} onClick={() => run((id) => client.continueExecution(id))}>{t.continue}</button>
          <button type="button" title={t.next} onClick={() => run((id) => client.next(id))}>{t.next}</button>
          <button type="button" title={t.stepIn} onClick={() => run((id) => client.stepIn(id))}>{t.stepIn}</button>
          <button type="button" title={t.stepOut} onClick={() => run((id) => client.stepOut(id))}>{t.stepOut}</button>
          <button type="button" title={t.pause} onClick={() => run((id) => client.pause(id))}>{t.pause}</button>
          <button type="button" onClick={() => void stop()}>{t.stop}</button>
        </> : <button type="button" onClick={() => void start()}>{t.start}</button>}
      </div>
      <div className="ide-debug-content">
        <div className="ide-debug-frames">{frames.length ? frames.map((frame) => <button type="button" key={frame.id} onClick={() => { if (frame.source?.path) onNavigate(frame.source.path, frame.line) }}>{frame.name}<small>{stackLocation(t.stackLocation, frame)}</small></button>) : <span>{t.noFrames}</span>}</div>
        <pre className="ide-debug-console">{debugLog(logs, error)}</pre>
      </div>
    </section>
  )
}
