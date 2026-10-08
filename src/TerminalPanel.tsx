import { useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'

interface TerminalPanelProps {
  projectPath: string
  language: 'en' | 'ru'
  visible: boolean
  onClose: () => void
}

interface TerminalEvent {
  terminalId: string
  data: string
}

const labels = {
  en: { title: 'TERMINAL', close: 'Close terminal', loading: 'Starting PowerShell…', error: 'Could not start terminal' },
  ru: { title: 'ТЕРМИНАЛ', close: 'Закрыть терминал', loading: 'Запуск PowerShell…', error: 'Не удалось запустить терминал' },
}

export function TerminalPanel({ projectPath, language, visible, onClose }: TerminalPanelProps) {
  const t = labels[language]
  const hostRef = useRef<HTMLDivElement | null>(null)
  const terminalRef = useRef<Terminal | null>(null)
  const terminalIdRef = useRef<string | null>(null)
  const [error, setError] = useState('')
  const [ready, setReady] = useState(false)

  useEffect(() => {
    const host = hostRef.current
    if (!host || !projectPath) return
    const terminal = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontFamily: "Consolas, 'Courier New', monospace",
      fontSize: 13,
      theme: { background: '#181818', foreground: '#d4d4d4', cursor: '#aeafad' },
    })
    const fit = new FitAddon()
    terminal.loadAddon(fit)
    terminal.open(host)
    terminalRef.current = terminal
    let disposed = false
    let unlistenOutput: (() => void) | undefined
    let unlistenExit: (() => void) | undefined
    let disposeInput: { dispose: () => void } | undefined
    let observer: ResizeObserver | undefined

    void (async () => {
      try {
        unlistenOutput = await listen<TerminalEvent>('terminal-output', (event) => {
          if (event.payload.terminalId === terminalIdRef.current) terminal.write(event.payload.data)
        })
        unlistenExit = await listen<{ terminalId: string; code: number | null }>('terminal-exit', (event) => {
          if (event.payload.terminalId === terminalIdRef.current) {
            terminalIdRef.current = null
            setReady(false)
          }
        })
        if (disposed) return
        fit.fit()
        const id = await invoke<string>('create_terminal', {
          projectPath,
          cols: terminal.cols,
          rows: terminal.rows,
        })
        if (disposed) {
          await invoke('close_terminal', { terminalId: id }).catch(() => undefined)
          return
        }
        terminalIdRef.current = id
        setReady(true)
        disposeInput = terminal.onData((data) => {
          if (terminalIdRef.current) void invoke('write_terminal_input', { terminalId: terminalIdRef.current, data }).catch((cause) => setError(String(cause)))
        })
        observer = new ResizeObserver(() => {
          if (!host.clientWidth || !host.clientHeight) return
          fit.fit()
          if (terminalIdRef.current) {
            void invoke('resize_terminal', { terminalId: terminalIdRef.current, cols: terminal.cols, rows: terminal.rows }).catch(() => undefined)
          }
        })
        observer.observe(host)
      } catch (cause) {
        if (!disposed) setError(`${t.error}: ${String(cause)}`)
      }
    })()

    return () => {
      disposed = true
      observer?.disconnect()
      disposeInput?.dispose()
      unlistenOutput?.()
      unlistenExit?.()
      const id = terminalIdRef.current
      terminalIdRef.current = null
      if (id) void invoke('close_terminal', { terminalId: id }).catch(() => undefined)
      terminalRef.current = null
      terminal.dispose()
    }
  }, [projectPath, t.error])

  return (
    <section className={`ide-terminal-panel ${visible ? 'visible' : 'hidden'}`} aria-hidden={!visible}>
      <div className="ide-terminal-heading"><span>{t.title}</span><span className="ide-terminal-status">{error || (ready ? '' : t.loading)}</span><button type="button" className="ide-terminal-close" title={t.close} aria-label={t.close} onClick={onClose} /></div>
      <div className="ide-terminal-host" ref={hostRef} />
    </section>
  )
}
