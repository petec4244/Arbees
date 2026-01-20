import { useState, useMemo, useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronDown, ChevronUp, Activity, Filter, ArrowUpDown, CheckCircle } from 'lucide-react'
import { useWebSocket } from '../hooks/useWebSocket'
import WinProbChart from '../components/WinProbChart'

export default function LiveGames() {
  const { subscribe, lastMessage } = useWebSocket()
  const queryClient = useQueryClient()
  const [selectedSport, setSelectedSport] = useState<string>('ALL')
  const [sortBy, setSortBy] = useState<'time' | 'score'>('time')
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

    if (sortBy === 'time') {
      // Sort by status priority then time
      // This is a simplified sort logic
      result.sort((a, b) => b.game_id.localeCompare(a.game_id))
    }

    return result
  }, [games, selectedSport, sortBy])

  return (
    <div className="space-y-6 h-full flex flex-col">
      <div className="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
        <div>
          <h1 className="text-3xl font-bold">Live Games</h1>
          <span className="text-sm text-gray-400">{filteredGames.length} active games</span>
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
            onClick={() => setSortBy(prev => prev === 'time' ? 'score' : 'time')}
            className="flex items-center space-x-2 text-sm px-2 text-gray-300 hover:text-white"
          >
            <ArrowUpDown className="w-4 h-4" />
            <span>{sortBy === 'time' ? 'Time' : 'Score'}</span>
          </button>
        </div>
      </div>

      <div className="flex-1 overflow-y-auto min-h-0 pr-2 custom-scrollbar">
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4 pb-4">
          {isLoading && <p className="text-gray-400 animate-pulse">Loading games...</p>}
          {filteredGames.map((game: any) => (
            <GameCard
              key={game.game_id}
              game={game}
              onSubscribe={() => handleSubscribe(game.game_id)}
              isTracked={trackedGames.has(game.game_id)}
            />
          ))}
          {(!isLoading && filteredGames.length === 0) && (
            <div className="col-span-full flex flex-col items-center justify-center p-12 text-gray-500 bg-gray-800/50 rounded-lg border border-gray-700 border-dashed">
              <Activity className="w-12 h-12 mb-4 opacity-20" />
              <p>No active games matching criteria</p>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

function GameCard({ game, onSubscribe, isTracked }: { game: any; onSubscribe: () => void; isTracked: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const [prevHomeScore, setPrevHomeScore] = useState(game.home_score)
  const [prevAwayScore, setPrevAwayScore] = useState(game.away_score)

  // Detect score changes for animation
  const homeScoreChanged = game.home_score !== prevHomeScore
  const awayScoreChanged = game.away_score !== prevAwayScore

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

  const mockHistory = useMemo(() => {
    if (!state || !state.home_win_prob) return []
    const prob = state.home_win_prob * 100
    return Array.from({ length: 10 }, (_, i) => ({
      time: `${i * 5}m`,
      prob: Math.max(0, Math.min(100, prob + (Math.random() - 0.5) * 10))
    }))
  }, [state])

  return (
    <div className={`bg-gray-800 rounded-lg overflow-hidden transition-all duration-300 ${expanded ? 'col-span-1 md:col-span-2 row-span-2 ring-2 ring-green-500/50' : 'hover:bg-gray-750'}`}>
      <div className="p-4">
        {/* Header */}
        <div className="flex justify-between items-start mb-4">
          <div className="flex items-center space-x-2">
            <span className="text-xs font-bold bg-gray-900 border border-gray-700 px-2 py-0.5 rounded text-gray-300 uppercase">
              {game.sport}
            </span>
            <span className="text-xs text-red-400 font-semibold animate-pulse">● LIVE</span>
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
            <span className="text-gray-300 font-medium truncate max-w-[70%]">{game.away_team || 'Away'}</span>
            <span className={`text-3xl font-mono font-bold transition-all duration-500 ${awayScoreChanged ? 'text-green-400 scale-125' : 'text-white'}`}>
              {game.away_score}
            </span>
          </div>
          <div className="flex justify-between items-center group">
            <span className="text-white font-medium truncate max-w-[70%]">{game.home_team || 'Home'}</span>
            <span className={`text-3xl font-mono font-bold transition-all duration-500 ${homeScoreChanged ? 'text-green-400 scale-125' : 'text-white'}`}>
              {game.home_score}
            </span>
          </div>
        </div>

        {/* Game State Footer */}
        <div className="border-t border-gray-700 pt-3 flex justify-between items-end">
          <div className="text-sm text-gray-400">
            {state ? (
              <>
                <div className="font-mono text-white">Q{state.period} - {state.time_remaining}</div>
                {state.home_win_prob && (
                  <div className="mt-1 flex items-center space-x-2">
                    <span>Win Prob:</span>
                    <span className={`${state.home_win_prob > 0.5 ? 'text-green-400' : 'text-red-400'} font-bold`}>
                      {(state.home_win_prob * 100).toFixed(1)}%
                    </span>
                  </div>
                )}
              </>
            ) : (
              <span className="italic">Waiting for updates...</span>
            )}
          </div>

          <button
            onClick={(e) => { e.stopPropagation(); if (!isTracked) onSubscribe(); }}
            disabled={isTracked}
            className={`px-3 py-1.5 text-xs font-medium rounded transition-all flex items-center space-x-1 ${isTracked
                ? 'bg-green-500/20 text-green-400 cursor-default'
                : 'bg-blue-600 hover:bg-blue-500 text-white'
              }`}
          >
            {isTracked && <CheckCircle className="w-3 h-3" />}
            <span>{isTracked ? 'Tracking' : 'Track'}</span>
          </button>
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

            <WinProbChart
              data={mockHistory}
              homeTeam={game.home_team}
              awayTeam={game.away_team}
            />
          </div>
        </div>
      )}
    </div>
  )
}
