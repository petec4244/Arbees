import { useEffect, useRef, useState, useCallback } from 'react'
import { create } from 'zustand'

interface WebSocketState {
  isConnected: boolean
  lastMessage: any
  setConnected: (connected: boolean) => void
  setLastMessage: (message: any) => void
}

export const useWebSocketStore = create<WebSocketState>((set) => ({
  isConnected: false,
  lastMessage: null,
  setConnected: (connected) => set({ isConnected: connected }),
  setLastMessage: (message) => set({ lastMessage: message }),
}))

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null)
  const reconnectTimeoutRef = useRef<number>()
  const { isConnected, lastMessage, setConnected, setLastMessage } = useWebSocketStore()

  const connect = useCallback(() => {
    if (wsRef.current?.readyState === WebSocket.OPEN) return

    const ws = new WebSocket(`ws://${window.location.host}/ws`)

    ws.onopen = () => {
      console.log('WebSocket connected')
      setConnected(true)
    }

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data)
        setLastMessage(data)
      } catch (e) {
        console.error('Failed to parse WebSocket message:', e)
      }
    }

    ws.onclose = () => {
      console.log('WebSocket disconnected')
      setConnected(false)
      // Reconnect after 3 seconds
      reconnectTimeoutRef.current = window.setTimeout(connect, 3000)
    }

    ws.onerror = (error) => {
      console.error('WebSocket error:', error)
    }

    wsRef.current = ws
  }, [setConnected, setLastMessage])

  useEffect(() => {
    connect()

    return () => {
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current)
      }
      wsRef.current?.close()
    }
  }, [connect])

  const send = useCallback((data: any) => {
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(data))
    }
  }, [])

  const subscribe = useCallback((gameId: string) => {
    send({ type: 'subscribe', game_id: gameId })
  }, [send])

  return { isConnected, lastMessage, send, subscribe }
}
