import { useState, useMemo, useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronUp, Activity, Filter, ArrowUpDown, CheckCircle } from 'lucide-react'
import { useWebSocket } from '../hooks/useWebSocket'
import GameTracker from '../components/GameTracker'
import { getMarketConfig, MarketBackground } from '../utils/board_config'

export default function LiveGames() {
  const { subscribe, lastMessage } = useWebSocket()
  const queryClient = useQueryClient()
  const [selectedSport, setSelectedSport] = useState<string>('ALL')
  const [sortBy, setSortBy] = useState<'time' | 'score' | 'sport'>('time')
  const [pinnedGames, setPinnedGames] = useState<Set<string>>(new Set())
  const [trackedGames, setTrackedGames] = useState<Set<string>>(new Set())

  // Handle incoming WebSocket messages
  useEffect(() => {
    if (lastMessage && lastMessage.type === 'game_update') {
      const { game_id, data } = lastMessage
      // Instantly update the cache for this game
      queryClient.setQueryData(['gameState', game_id], data)
    }
  }, [lastMessage, queryClient])

  const { data: games, isLoading } = useQuery({
    queryKey: ['liveGames'],
    queryFn: async () => {
      const res = await fetch('/api/live-games')
      return res.json()
    },
    refetchInterval: 5000,
  })

  const handleSubscribe = (gameId: string) => {
    subscribe(gameId)
    setTrackedGames(prev => new Set(prev).add(gameId))
  }

  const togglePin = (gameId: string) => {
    setPinnedGames(prev => {
      const next = new Set(prev)
      if (next.has(gameId)) next.delete(gameId)
      else next.add(gameId)
      return next
    })
  }

  const sports = useMemo(() => {
    if (!games) return []
    const distinct = new Set(games.map((g: any) => g.sport))
    return ['ALL', ...Array.from(distinct)]
  }, [games])

  const filteredGames = useMemo(() => {
    if (!games) return []
    let result = [...games]

    if (selectedSport !== 'ALL') {
      result = result.filter(g => g.sport === selectedSport)
    }

    // Filter out finalized games older than 1 hour
    result = result.filter(g => {
      if (g.status === 'FINAL' || g.status === 'COMPLETED') {
        // If we have a timestamp, check if it's > 1 hour ago.
        // If no timestamp, default to keeping it (or maybe removing it if we can't tell).
        // Let's assume 'last_update' or 'timestamp' exists.
        // Based on code, `state.timestamp` exists. `game` usually has one too.
        const time = g.last_update || g.timestamp;
        if (time) {
          const age = Date.now() - new Date(time).getTime();
          return age < 3600000; // 1 hour
        }
        return true;
      }
      return true;
    })

    // Sort by pinned then by selected sort
    result.sort((a, b) => {
      const aPinned = pinnedGames.has(a.game_id)
      const bPinned = pinnedGames.has(b.game_id)
      if (aPinned && !bPinned) return -1
      if (!aPinned && bPinned) return 1

      if (sortBy === 'time') {
        return b.game_id.localeCompare(a.game_id)
      }
      if (sortBy === 'score') {
        return (b.home_score + b.away_score) - (a.home_score + a.away_score)
      }
      if (sortBy === 'sport') {
        return a.sport.localeCompare(b.sport)
      }
      return 0
    })

    return result
  }, [games, selectedSport, sortBy, pinnedGames])

  return (
    <div className="space-y-6 h-full flex flex-col">
      <div className="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
        <div>
          <h1 className="text-3xl font-bold">Live Events</h1>
          <span className="text-sm text-gray-400">{filteredGames.length} active events</span>
        </div>

        <div className="flex items-center space-x-3 bg-gray-800 p-2 rounded-lg">
          <div className="flex items-center space-x-2 px-2">
            <Filter className="w-4 h-4 text-gray-400" />
            <select
              value={selectedSport}
              onChange={(e) => setSelectedSport(e.target.value)}
              className="bg-transparent text-sm focus:outline-none cursor-pointer"
            >
              {sports.map((s: any) => (
                <option key={s} value={s}>{s}</option>
              ))}
            </select>
          </div>
          <div className="w-px h-4 bg-gray-700" />
          <button
            onClick={() => setSortBy(prev => {
              if (prev === 'time') return 'score'
              if (prev === 'score') return 'sport'
              return 'time'
            })}
            className="flex items-center space-x-2 text-sm px-2 text-gray-300 hover:text-white"
          >
            <ArrowUpDown className="w-4 h-4" />
            <span>{sortBy === 'time' ? 'Time' : sortBy === 'score' ? 'Score' : 'Type'}</span>
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto min-h-0 pr-2 custom-scrollbar">
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4 pb-4">
          {isLoading && <p className="text-gray-400 animate-pulse">Loading events...</p>}
          {filteredGames.map((game: any) => (
            <GameCard
              key={game.game_id}
              game={game}
              onSubscribe={() => handleSubscribe(game.game_id)}
              isTracked={trackedGames.has(game.game_id)}
              isPinned={pinnedGames.has(game.game_id)}
              onTogglePin={() => togglePin(game.game_id)}
            />
          ))}
          {(!isLoading && filteredGames.length === 0) && (
            <div className="col-span-full flex flex-col items-center justify-center p-12 text-gray-500 bg-gray-800/50 rounded-lg border border-gray-700 border-dashed">
              <Activity className="w-12 h-12 mb-4 opacity-20" />
              <p>No active events matching criteria</p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}


function formatDataAge(timestamp?: string) {
  if (!timestamp) return 'No data'
  const age = Date.now() - new Date(timestamp).getTime()
  if (age < 2000) return 'Just now'
  if (age < 5000) return 'Active'
  if (age < 30000) return `${Math.floor(age / 1000)}s ago`
  return `${Math.floor(age / 60000)}m ago`
}

function GameCard({ game, onSubscribe, isTracked, isPinned, onTogglePin }: { game: any; onSubscribe: () => void; isTracked: boolean; isPinned: boolean; onTogglePin: () => void }) {
  const [expanded, setExpanded] = useState(false)
  const [prevHomeScore, setPrevHomeScore] = useState(game.home_score)
  const [prevAwayScore, setPrevAwayScore] = useState(game.away_score)

  // Detect score changes for animation
  const homeScoreChanged = game.home_score !== prevHomeScore
  const awayScoreChanged = game.away_score !== prevAwayScore

  const isFinal = game.status === 'FINAL' || game.status === 'COMPLETED'

  if (homeScoreChanged) setTimeout(() => setPrevHomeScore(game.home_score), 2000)
  if (awayScoreChanged) setTimeout(() => setPrevAwayScore(game.away_score), 2000)

  const { data: state } = useQuery({
    queryKey: ['gameState', game.game_id],
    queryFn: async () => {
      const res = await fetch(`/api/live-games/${game.game_id}/state`)
      if (!res.ok) return null
      return res.json()
    },
    enabled: !!game.game_id,
    refetchInterval: expanded ? 2000 : 10000,
  })

  const { data: history } = useQuery({
    queryKey: ['gameHistory', game.game_id],
    queryFn: async () => {
      const res = await fetch(`/api/live-games/${game.game_id}/history`)
      if (!res.ok) return []
      const data = await res.json()
      // Format for chart (raw values for GameTracker)
      return data.map((d: any) => ({
        timestamp: new Date(d.time).getTime(),
        homeValue: d.home_win_prob * 100,
        awayValue: (1 - d.home_win_prob) * 100
      }))
    },
    enabled: expanded && !!game.game_id,
    staleTime: 60000, // Cache for 1 minute
  })

  // Fetch trades for this game
  const { data: trades } = useQuery({
    queryKey: ['gameTrades', game.game_id],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/trades')
      if (!res.ok) return []
      const allTrades = await res.json()
      // Filter for this game and map to chart format
      return allTrades
        .filter((t: any) => t.game_id === game.game_id)
        .map((t: any) => ({
          id: t.trade_id,
          entryTime: t.entry_time,
          entryPrice: t.entry_price,
          side: t.side,
          pnl: t.pnl,
          // Heuristic: If buying Home Win Prob, you bet Home. If selling (shorting), you bet Away.
          team: t.side === 'buy' ? 'home' : 'away'
        }))
    },
    enabled: expanded,
    staleTime: 30000
  })

  const sportConfig = getMarketConfig(game.sport)

  return (
    <div className={`rounded-lg overflow-hidden transition-all duration-300 relative group border ${sportConfig.colors} ${isFinal ? 'opacity-60 grayscale' : ''} ${expanded ? 'col-span-1 md:col-span-2 row-span-2 ring-2 ring-green-500/50' : ''}`}>
      <MarketBackground type={game.sport} />
      <div className="p-4 relative z-10">
        {/* Header */}
        <div className="flex justify-between items-start mb-4">
          <div className="flex flex-col">
            <div className="flex items-center space-x-2">
              <span className={`text-xs font-bold px-2 py-0.5 rounded uppercase border ${sportConfig.badge}`}>
                {game.sport}
              </span>
              <button onClick={(e) => { e.stopPropagation(); onTogglePin(); }} className={`text-xs px-2 py-0.5 rounded border transition-colors ${isPinned ? 'bg-yellow-500/20 text-yellow-400 border-yellow-500/50' : 'bg-gray-800 text-gray-500 border-gray-700 hover:text-gray-300'}`}>
                {isPinned ? '★ PINNED' : '☆ PIN'}
              </button>
              {isFinal ? (
                <span className="text-xs text-gray-400 font-bold border border-gray-600 px-2 py-0.5 rounded">FINAL</span>
              ) : (
                <span className="text-xs text-red-400 font-semibold animate-pulse">● LIVE</span>
              )}
              {game.cooldown_until && new Date(game.cooldown_until) > new Date() && (
                <span className="text-[10px] bg-blue-900/50 text-blue-300 border border-blue-700/50 px-1.5 py-0.5 rounded flex items-center gap-1">
                  ❄️ {Math.ceil((new Date(game.cooldown_until).getTime() - Date.now()) / 60000)}m
                </span>
              )}
            </div>
            {state?.timestamp && (
              <span className="text-[10px] text-gray-500 mt-1 ml-1" title={`Updated: ${new Date(state.timestamp).toLocaleTimeString()}`}>
                Age: {formatDataAge(state.timestamp)}
              </span>
            )}
          </div>
          <button
            onClick={() => setExpanded(!expanded)}
            className="text-gray-400 hover:text-white transition-colors"
          >
            {expanded ? <ChevronUp className="w-5 h-5" /> : <ChevronDown className="w-5 h-5" />}
          </button>
        </div>

        {/* Scores */}
        <div className="space-y-3 mb-4">
          <div className="flex justify-between items-center group">
            <span className="text-blue-400 font-medium truncate max-w-[70%]">{game.away_team || 'Away'}</span>
            <span className={`text-3xl font-mono font-bold transition-all duration-500 ${awayScoreChanged ? 'text-green-400 scale-125' : 'text-white'}`}>
              {game.away_score}
            </span>
          </div>
          <div className="flex justify-between items-center group">
            <span className="text-green-400 font-medium truncate max-w-[70%]">{game.home_team || 'Home'}</span>
            <span className={`text-3xl font-mono font-bold transition-all duration-500 ${homeScoreChanged ? 'text-green-400 scale-125' : 'text-white'}`}>
              {game.home_score}
            </span>
          </div>
        </div>

        {/* Game State Footer */}
        <div className="border-t border-gray-700 pt-3 flex justify-between items-end">
          <div className="text-sm text-gray-400 w-full">
            {state ? (
              <>
                <div className="font-mono text-white mb-2">Q{state.period} - {state.time_remaining}</div>
                <div className="flex justify-between items-center">
                  {/* Home Win Prob + Active Bets */}
                  <div className="flex items-center space-x-2">
                    <span className="text-xs text-gray-500">Home Prob:</span>
                    <span className="text-green-400 font-bold font-mono">
                      {(state.home_win_prob * 100).toFixed(1)}%
                    </span>
                    {/* Fake active bets circles for demo - in real app, derive from 'trades' */}
                    <div className="flex space-x-1">
                      {/* Example: 2 positive bets */}
                      {trades && trades.filter((t: any) => t.team === 'home' && (t.pnl || 0) >= 0).map((t: any) => (
                        <div key={t.id} className="w-2 h-2 rounded-full bg-green-500" title="Winning Home Bet" />
                      ))}
                      {trades && trades.filter((t: any) => t.team === 'home' && (t.pnl || 0) < 0).map((t: any) => (
                        <div key={t.id} className="w-2 h-2 rounded-full bg-green-900 border border-green-700" title="Losing Home Bet" />
                      ))}
                    </div>
                  </div>

                  {/* Away "Prob" (1-Home) + Active Bets */}
                  <div className="flex items-center space-x-2">
                    <span className="text-xs text-gray-500">Away Prob:</span>
                    <span className="text-blue-400 font-bold font-mono">
                      {((1 - state.home_win_prob) * 100).toFixed(1)}%
                    </span>
                    <div className="flex space-x-1">
                      {/* Example: Away bets */}
                      {trades && trades.filter((t: any) => t.team === 'away' && (t.pnl || 0) >= 0).map((t: any) => (
                        <div key={t.id} className="w-2 h-2 rounded-full bg-green-500" title="Winning Away Bet" />
                      ))}
                      {trades && trades.filter((t: any) => t.team === 'away' && (t.pnl || 0) < 0).map((t: any) => (
                        <div key={t.id} className="w-2 h-2 rounded-full bg-green-900 border border-green-700" title="Losing Away Bet" />
                      ))}
                    </div>
                  </div>
                </div>
              </>
            ) : (
              <span className="italic">Waiting for updates...</span>
            )}
          </div>
        </div>
      </div>

      {/* Expanded Content (Chart) */}
      {expanded && (
        <div className="px-4 pb-4 animate-in slide-in-from-top-4 fade-in duration-300">
          <div className="bg-gray-900/50 rounded-lg p-3 border border-gray-700/50">
            <div className="flex items-center space-x-2 mb-2">
              <Activity className="w-4 h-4 text-green-400" />
              <span className="text-xs font-bold text-gray-300 uppercase">Win Probability Momentum</span>
            </div>

            <GameTracker
              history={history || []}
              homeTeam={game.home_team}
              awayTeam={game.away_team}
              title="Win Probability"
              trades={trades || []}
              cooldownUntil={game.cooldown_until}
            />
          </div>
        </div>
      )}
    </div>
  )
}
