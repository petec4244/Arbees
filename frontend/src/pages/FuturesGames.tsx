import { useState, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import {
  Clock,
  Filter,
  TrendingUp,
  TrendingDown,
  BarChart3,
  Activity,
  AlertCircle,
  Timer,
  Eye,
  X,
  LineChart,
  Target,
  ArrowUpRight,
  ArrowDownRight,
  Check,
  DollarSign,
  ArrowUpDown,
} from 'lucide-react'
import { getSportConfig, SportBackground } from '../utils/sports'

// Types
interface FuturesGame {
  game_id: string
  sport: string
  home_team: string
  away_team: string
  scheduled_time: string
  hours_until_start: number
  has_kalshi: boolean
  has_polymarket: boolean
  opening_home_prob: number | null
  current_home_prob: number | null
  line_movement_pct: number | null
  movement_direction: string | null
  total_volume: number
  active_signals: number
  lifecycle_status: string
}

interface FuturesSignal {
  signal_id: string
  game_id: string
  sport: string
  signal_type: string
  direction: string
  edge_pct: number
  confidence: number | null
  hours_until_start: number | null
  reason: string | null
  executed: boolean
  time: string
}

interface FuturesPricePoint {
  time: string
  platform: string
  market_type: string
  yes_mid: number | null
  spread_cents: number | null
  volume: number | null
  hours_until_start: number | null
}

interface FuturesStats {
  total_monitored: number
  games_with_markets: number
  active_signals: number
  avg_line_movement: number
  by_sport: Record<string, number>
}

// Sport color map - REPLACED by utils/sports
// const sportColors: Record<string, string> = { ... }

const TIME_WINDOW_OPTIONS = [
  { value: 12, label: '12 hours' },
  { value: 24, label: '24 hours' },
  { value: 36, label: '36 hours' },
  { value: 48, label: '48 hours' },
]

export default function FuturesGames() {
  const [selectedSport, setSelectedSport] = useState<string>('ALL')
  const [maxHours, setMaxHours] = useState<number>(48)
  const [selectedGame, setSelectedGame] = useState<FuturesGame | null>(null)
  const [sortBy, setSortBy] = useState<'time' | 'sport' | 'movement'>('movement')

  // Fetch futures games
  const { data: games, isLoading, isError } = useQuery<FuturesGame[]>({
    queryKey: ['futuresGames', selectedSport, maxHours],
    queryFn: async () => {
      const params = new URLSearchParams({
        max_hours: maxHours.toString(),
        limit: '100',
      })
      if (selectedSport !== 'ALL') {
        params.append('sport', selectedSport.toLowerCase())
      }
      const res = await fetch(`/api/futures/games?${params}`)
      if (!res.ok) throw new Error('Failed to fetch futures games')
      return res.json()
    },
    refetchInterval: 30000, // Refresh every 30 seconds
  })

  // Fetch futures stats
  const { data: stats } = useQuery<FuturesStats>({
    queryKey: ['futuresStats'],
    queryFn: async () => {
      const res = await fetch('/api/futures/stats')
      if (!res.ok) throw new Error('Failed to fetch stats')
      return res.json()
    },
    refetchInterval: 60000,
  })

  // Fetch active signals
  const { data: signals } = useQuery<FuturesSignal[]>({
    queryKey: ['futuresSignals'],
    queryFn: async () => {
      const res = await fetch('/api/futures/signals?min_edge=3&limit=20')
      if (!res.ok) throw new Error('Failed to fetch signals')
      return res.json()
    },
    refetchInterval: 30000,
  })

  // Get unique sports from stats
  const [sortedGames, setSortedGames] = useState<FuturesGame[]>([])

  // Sort games whenever games or sortBy changes
  useMemo(() => {
    if (!games) {
      setSortedGames([])
      return
    }

    const sorted = [...games].sort((a, b) => {
      if (sortBy === 'time') {
        return new Date(a.scheduled_time).getTime() - new Date(b.scheduled_time).getTime()
      }
      if (sortBy === 'sport') {
        return a.sport.localeCompare(b.sport)
      }
      if (sortBy === 'movement') {
        // Sort by absolute line movement descending
        const movA = Math.abs(a.line_movement_pct || 0)
        const movB = Math.abs(b.line_movement_pct || 0)
        return movB - movA
      }
      return 0
    })
    setSortedGames(sorted)
  }, [games, sortBy])

  // Get unique sports from stats
  const sports = useMemo(() => {
    if (stats?.by_sport) {
      return ['ALL', ...Object.keys(stats.by_sport).sort()]
    }
    return ['ALL']
  }, [stats])

  const getSportClass = (sport: string) => {
    return getSportConfig(sport).badge
  }

  const formatHoursUntil = (hours: number) => {
    if (hours < 1) {
      return `${Math.round(hours * 60)}m`
    } else if (hours < 24) {
      return `${hours.toFixed(1)}h`
    } else {
      const days = Math.floor(hours / 24)
      const remainingHours = Math.round(hours % 24)
      return `${days}d ${remainingHours}h`
    }
  }

  return (
    <div className="space-y-6 h-full flex flex-col">
      {/* Header */}
      <div className="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
        <div>
          <h1 className="text-3xl font-bold flex items-center gap-3">
            <LineChart className="w-8 h-8 text-purple-400" />
            Futures Monitoring
          </h1>
          <span className="text-sm text-gray-400">
            Pre-game market tracking and line movement analysis
          </span>
        </div>

        {/* Filters */}
        <div className="flex items-center space-x-3 bg-gray-800 p-2 rounded-lg">
          {/* Sport Filter */}
          <div className="flex items-center space-x-2 px-2">
            <Filter className="w-4 h-4 text-gray-400" />
            <select
              value={selectedSport}
              onChange={(e) => setSelectedSport(e.target.value)}
              className="bg-transparent text-sm focus:outline-none cursor-pointer"
            >
              {sports.map((s) => (
                <option key={s} value={s} className="bg-gray-800">
                  {s.toUpperCase()} {stats?.by_sport[s] ? `(${stats.by_sport[s]})` : ''}
                </option>
              ))}
            </select>
          </div>

          <div className="w-px h-4 bg-gray-700" />

          {/* Time Window */}
          <div className="flex items-center space-x-2 px-2">
            <Clock className="w-4 h-4 text-gray-400" />
            <select
              value={maxHours}
              onChange={(e) => setMaxHours(Number(e.target.value))}
              className="bg-transparent text-sm focus:outline-none cursor-pointer"
            >
              {TIME_WINDOW_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value} className="bg-gray-800">
                  {opt.label}
                </option>
              ))}
            </select>
          </div>

          <div className="w-px h-4 bg-gray-700" />

          {/* Sort Control */}
          <button
            onClick={() => setSortBy(prev => {
              if (prev === 'movement') return 'time'
              if (prev === 'time') return 'sport'
              return 'movement'
            })}
            className="flex items-center space-x-2 px-2 text-gray-400 hover:text-white transition-colors"
          >
            <ArrowUpDown className="w-4 h-4" />
            <span className="text-sm">
              {sortBy === 'movement' ? 'Movement' : sortBy === 'time' ? 'Time' : 'Sport'}
            </span>
          </button>
        </div>
      </div>

      {/* Stats Summary */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
        <StatsCard
          icon={<Eye className="w-5 h-5 text-purple-400" />}
          title="Monitored Games"
          value={stats?.total_monitored || 0}
        />
        <StatsCard
          icon={<BarChart3 className="w-5 h-5 text-blue-400" />}
          title="With Markets"
          value={stats?.games_with_markets || 0}
          subtitle={stats ? `${Math.round((stats.games_with_markets / Math.max(1, stats.total_monitored)) * 100)}% discovery` : undefined}
        />
        <StatsCard
          icon={<Activity className="w-5 h-5 text-yellow-400" />}
          title="Active Signals"
          value={stats?.active_signals || 0}
        />
        <StatsCard
          icon={<TrendingUp className="w-5 h-5 text-green-400" />}
          title="Avg Line Movement"
          value={`${(stats?.avg_line_movement || 0).toFixed(1)}%`}
        />
      </div>

      {/* Main Content Grid */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6 flex-1 min-h-0">
        {/* Games List */}
        <div className="lg:col-span-2 bg-gray-800 rounded-lg overflow-hidden flex flex-col">
          <div className="p-4 border-b border-gray-700">
            <h2 className="font-bold text-lg">Monitored Games</h2>
          </div>

          <div className="flex-1 overflow-y-auto">
            {isLoading && (
              <div className="flex items-center justify-center p-12 text-gray-400">
                <div className="animate-spin w-6 h-6 border-2 border-purple-400 border-t-transparent rounded-full mr-3" />
                Loading futures games...
              </div>
            )}

            {isError && (
              <div className="flex flex-col items-center justify-center p-12 text-red-400">
                <AlertCircle className="w-8 h-8 mb-2" />
                <p>Failed to load futures games</p>
              </div>
            )}

            {!isLoading && !isError && games?.length === 0 && (
              <div className="flex flex-col items-center justify-center p-12 text-gray-500">
                <Eye className="w-12 h-12 mb-4 opacity-20" />
                <p>No games being monitored</p>
              </div>
            )}

            {!isLoading && !isError && sortedGames && sortedGames.length > 0 && (
              <div className="divide-y divide-gray-700">
                {sortedGames.map((game) => (
                  <GameRow
                    key={game.game_id}
                    game={game}
                    onClick={() => setSelectedGame(game)}
                    formatHoursUntil={formatHoursUntil}
                    getSportClass={getSportClass}
                  />
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Signals Panel */}
        <div className="bg-gray-800 rounded-lg overflow-hidden flex flex-col">
          <div className="p-4 border-b border-gray-700">
            <h2 className="font-bold text-lg flex items-center gap-2">
              <Target className="w-5 h-5 text-yellow-400" />
              Active Signals
            </h2>
          </div>

          <div className="flex-1 overflow-y-auto">
            {!signals || signals.length === 0 ? (
              <div className="flex flex-col items-center justify-center p-8 text-gray-500">
                <Target className="w-8 h-8 mb-2 opacity-20" />
                <p className="text-sm">No active signals</p>
              </div>
            ) : (
              <div className="divide-y divide-gray-700">
                {signals.map((signal) => (
                  <SignalRow
                    key={signal.signal_id}
                    signal={signal}
                    getSportClass={getSportClass}
                    formatHoursUntil={formatHoursUntil}
                  />
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* Game Detail Modal */}
      {selectedGame && (
        <GameDetailModal
          game={selectedGame}
          onClose={() => setSelectedGame(null)}
          formatHoursUntil={formatHoursUntil}
          getSportClass={getSportClass}
        />
      )}
    </div>
  )
}

function StatsCard({
  icon,
  title,
  value,
  subtitle,
}: {
  icon: React.ReactNode
  title: string
  value: string | number
  subtitle?: string
}) {
  return (
    <div className="bg-gray-800 rounded-lg p-4 border border-gray-700">
      <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
        {icon}
        <span>{title}</span>
      </div>
      <p className="text-2xl font-bold">{value}</p>
      {subtitle && <p className="text-xs text-gray-500 mt-1">{subtitle}</p>}
    </div>
  )
}

function GameRow({
  game,
  onClick,
  formatHoursUntil,
  getSportClass,
}: {
  game: FuturesGame
  onClick: () => void
  formatHoursUntil: (hours: number) => string
  getSportClass: (sport: string) => string
}) {
  const movementColor = game.line_movement_pct
    ? Math.abs(game.line_movement_pct) >= 3
      ? 'text-yellow-400'
      : 'text-gray-400'
    : 'text-gray-500'

  const sportConfig = getSportConfig(game.sport)

  return (
    <div
      onClick={onClick}
      className={`p-4 relative overflow-hidden group hover:bg-gray-700/50 cursor-pointer transition-colors border-l-4 ${sportConfig.colors.replace('border-', 'border-l-')}`}
      style={{ borderLeftColor: 'var(--tw-border-opacity)' }} // Fallback/hack if generic border class doesn't mapped well to border-l. Actually let's just use the config's color for border-l manually or just the bg.
    >
      <SportBackground sport={game.sport} />
      <div className="relative z-10">
        <div className="flex justify-between items-start mb-2">
          <div className="flex items-center gap-2">
            <span className={`px-2 py-0.5 rounded text-xs font-medium uppercase border ${getSportClass(game.sport)}`}>
              {game.sport}
            </span>
            <span className="text-sm text-gray-400">
              <Timer className="w-3 h-3 inline mr-1" />
              {formatHoursUntil(game.hours_until_start)}
            </span>
          </div>
          <div className="flex items-center gap-2">
            {game.has_kalshi && (
              <span className="px-1.5 py-0.5 rounded bg-blue-900/50 text-blue-300 text-xs font-medium">K</span>
            )}
            {game.has_polymarket && (
              <span className="px-1.5 py-0.5 rounded bg-purple-900/50 text-purple-300 text-xs font-medium">P</span>
            )}
          </div>
        </div>

        <div className="font-medium mb-2">
          {game.away_team} @ {game.home_team}
        </div>

        <div className="flex items-center justify-between text-sm">
          <div className="flex items-center gap-4">
            {game.current_home_prob && (
              <span className="text-gray-400">
                Home: <span className="font-mono text-white">{(game.current_home_prob * 100).toFixed(1)}%</span>
              </span>
            )}
            {game.line_movement_pct !== null && (
              <span className={movementColor}>
                {game.movement_direction === 'home' ? (
                  <ArrowUpRight className="w-3 h-3 inline" />
                ) : (
                  <ArrowDownRight className="w-3 h-3 inline" />
                )}
                {Math.abs(game.line_movement_pct).toFixed(1)}%
              </span>
            )}
          </div>
          {game.active_signals > 0 && (
            <span className="px-2 py-0.5 rounded-full bg-yellow-500/20 text-yellow-400 text-xs font-medium">
              {game.active_signals} signal{game.active_signals > 1 ? 's' : ''}
            </span>
          )}
        </div>
      </div>
    </div>
  )
}

function SignalRow({
  signal,
  getSportClass,
  formatHoursUntil,
}: {
  signal: FuturesSignal
  getSportClass: (sport: string) => string
  formatHoursUntil: (hours: number) => string
}) {
  const isEarlyEdge = signal.signal_type === 'futures_early_edge'
  const isLineMovement = signal.signal_type === 'futures_line_movement'

  return (
    <div className="p-3">
      <div className="flex justify-between items-start mb-1">
        <span className={`px-1.5 py-0.5 rounded text-xs font-medium uppercase border ${getSportClass(signal.sport)}`}>
          {signal.sport}
        </span>
        <span className={`text-xs font-bold ${signal.edge_pct >= 5 ? 'text-green-400' : 'text-yellow-400'}`}>
          {signal.edge_pct.toFixed(1)}% edge
        </span>
      </div>

      <div className="flex items-center gap-2 mb-1">
        <span className={`px-1.5 py-0.5 rounded text-xs font-medium ${isEarlyEdge
          ? 'bg-blue-900/50 text-blue-300'
          : isLineMovement
            ? 'bg-purple-900/50 text-purple-300'
            : 'bg-gray-700 text-gray-300'
          }`}>
          {isEarlyEdge ? 'Cross-Platform' : isLineMovement ? 'Line Movement' : signal.signal_type}
        </span>
        <span className={`px-1.5 py-0.5 rounded text-xs font-medium ${signal.direction === 'yes'
          ? 'bg-green-900/50 text-green-300'
          : 'bg-red-900/50 text-red-300'
          }`}>
          {signal.direction.toUpperCase()}
        </span>
        {signal.executed && (
          <Check className="w-3 h-3 text-green-400" />
        )}
      </div>

      <div className="text-xs text-gray-400 truncate">
        {signal.reason || signal.game_id}
      </div>

      <div className="text-xs text-gray-500 mt-1">
        {signal.hours_until_start !== null && (
          <span>{formatHoursUntil(signal.hours_until_start)} until start</span>
        )}
      </div>
    </div>
  )
}

function GameDetailModal({
  game,
  onClose,
  formatHoursUntil,
  getSportClass,
}: {
  game: FuturesGame
  onClose: () => void
  formatHoursUntil: (hours: number) => string
  getSportClass: (sport: string) => string
}) {
  // Fetch price history for the selected game
  const { data: priceHistory, isLoading: priceLoading } = useQuery<FuturesPricePoint[]>({
    queryKey: ['futuresPriceHistory', game.game_id],
    queryFn: async () => {
      const res = await fetch(`/api/futures/games/${game.game_id}/prices?limit=500`)
      if (!res.ok) throw new Error('Failed to fetch price history')
      return res.json()
    },
  })

  // Group price history by platform
  const kalshiPrices = priceHistory?.filter(p => p.platform === 'kalshi') || []
  const polyPrices = priceHistory?.filter(p => p.platform === 'polymarket') || []

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50">
      <div className="bg-gray-800 rounded-lg w-full max-w-3xl max-h-[90vh] overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex justify-between items-center p-4 border-b border-gray-700">
          <div>
            <div className="flex items-center gap-2 mb-1">
              <span className={`px-2 py-0.5 rounded text-xs font-medium uppercase border ${getSportClass(game.sport)}`}>
                {game.sport}
              </span>
              <span className="text-sm text-gray-400">
                Starts in {formatHoursUntil(game.hours_until_start)}
              </span>
            </div>
            <h2 className="text-xl font-bold">
              {game.away_team} @ {game.home_team}
            </h2>
          </div>
          <button
            onClick={onClose}
            className="p-2 hover:bg-gray-700 rounded-lg transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Stats */}
        <div className="grid grid-cols-4 gap-4 p-4 border-b border-gray-700">
          <div className="text-center">
            <p className="text-sm text-gray-400">Opening</p>
            <p className="text-xl font-bold font-mono">
              {game.opening_home_prob ? `${(game.opening_home_prob * 100).toFixed(1)}%` : '-'}
            </p>
          </div>
          <div className="text-center">
            <p className="text-sm text-gray-400">Current</p>
            <p className="text-xl font-bold font-mono">
              {game.current_home_prob ? `${(game.current_home_prob * 100).toFixed(1)}%` : '-'}
            </p>
          </div>
          <div className="text-center">
            <p className="text-sm text-gray-400">Movement</p>
            <p className={`text-xl font-bold font-mono ${game.line_movement_pct && Math.abs(game.line_movement_pct) >= 3
              ? game.movement_direction === 'home' ? 'text-green-400' : 'text-red-400'
              : ''
              }`}>
              {game.line_movement_pct !== null ? (
                <>
                  {game.movement_direction === 'home' ? '+' : '-'}
                  {Math.abs(game.line_movement_pct).toFixed(1)}%
                </>
              ) : '-'}
            </p>
          </div>
          <div className="text-center">
            <p className="text-sm text-gray-400">Volume</p>
            <p className="text-xl font-bold font-mono">
              <DollarSign className="w-4 h-4 inline" />
              {game.total_volume >= 1000
                ? `${(game.total_volume / 1000).toFixed(0)}k`
                : game.total_volume.toFixed(0)}
            </p>
          </div>
        </div>

        {/* Market Status */}
        <div className="p-4 border-b border-gray-700">
          <h3 className="text-sm font-medium text-gray-400 mb-2">Market Discovery</h3>
          <div className="flex gap-4">
            <div className={`flex items-center gap-2 px-3 py-2 rounded-lg ${game.has_kalshi ? 'bg-blue-900/30 border border-blue-500/30' : 'bg-gray-700/50'
              }`}>
              <span className={game.has_kalshi ? 'text-blue-400' : 'text-gray-500'}>Kalshi</span>
              {game.has_kalshi ? (
                <Check className="w-4 h-4 text-blue-400" />
              ) : (
                <X className="w-4 h-4 text-gray-500" />
              )}
            </div>
            <div className={`flex items-center gap-2 px-3 py-2 rounded-lg ${game.has_polymarket ? 'bg-purple-900/30 border border-purple-500/30' : 'bg-gray-700/50'
              }`}>
              <span className={game.has_polymarket ? 'text-purple-400' : 'text-gray-500'}>Polymarket</span>
              {game.has_polymarket ? (
                <Check className="w-4 h-4 text-purple-400" />
              ) : (
                <X className="w-4 h-4 text-gray-500" />
              )}
            </div>
          </div>
        </div>

        {/* Price History */}
        <div className="flex-1 p-4 overflow-y-auto">
          <h3 className="text-sm font-medium text-gray-400 mb-3">Price History</h3>

          {priceLoading ? (
            <div className="flex items-center justify-center p-8 text-gray-400">
              <div className="animate-spin w-5 h-5 border-2 border-purple-400 border-t-transparent rounded-full mr-2" />
              Loading prices...
            </div>
          ) : (!priceHistory || priceHistory.length === 0) ? (
            <div className="flex items-center justify-center p-8 text-gray-500">
              <LineChart className="w-8 h-8 mr-2 opacity-20" />
              No price history yet
            </div>
          ) : (
            <div className="space-y-4">
              {/* Kalshi Prices */}
              {kalshiPrices.length > 0 && (
                <div>
                  <h4 className="text-xs font-medium text-blue-400 mb-2">Kalshi ({kalshiPrices.length} samples)</h4>
                  <div className="grid grid-cols-4 gap-2 text-xs">
                    <div className="text-gray-500">Time</div>
                    <div className="text-gray-500">Mid</div>
                    <div className="text-gray-500">Spread</div>
                    <div className="text-gray-500">Hours</div>
                    {kalshiPrices.slice(0, 10).map((p, i) => (
                      <>
                        <div key={`k-time-${i}`} className="text-gray-400 font-mono">
                          {new Date(p.time).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                        </div>
                        <div key={`k-mid-${i}`} className="font-mono">
                          {p.yes_mid ? `${(p.yes_mid * 100).toFixed(1)}%` : '-'}
                        </div>
                        <div key={`k-spread-${i}`} className="font-mono text-gray-400">
                          {p.spread_cents ? `${p.spread_cents.toFixed(1)}c` : '-'}
                        </div>
                        <div key={`k-hours-${i}`} className="font-mono text-gray-400">
                          {p.hours_until_start ? formatHoursUntil(p.hours_until_start) : '-'}
                        </div>
                      </>
                    ))}
                  </div>
                </div>
              )}

              {/* Polymarket Prices */}
              {polyPrices.length > 0 && (
                <div>
                  <h4 className="text-xs font-medium text-purple-400 mb-2">Polymarket ({polyPrices.length} samples)</h4>
                  <div className="grid grid-cols-4 gap-2 text-xs">
                    <div className="text-gray-500">Time</div>
                    <div className="text-gray-500">Mid</div>
                    <div className="text-gray-500">Spread</div>
                    <div className="text-gray-500">Hours</div>
                    {polyPrices.slice(0, 10).map((p, i) => (
                      <>
                        <div key={`p-time-${i}`} className="text-gray-400 font-mono">
                          {new Date(p.time).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                        </div>
                        <div key={`p-mid-${i}`} className="font-mono">
                          {p.yes_mid ? `${(p.yes_mid * 100).toFixed(1)}%` : '-'}
                        </div>
                        <div key={`p-spread-${i}`} className="font-mono text-gray-400">
                          {p.spread_cents ? `${p.spread_cents.toFixed(1)}c` : '-'}
                        </div>
                        <div key={`p-hours-${i}`} className="font-mono text-gray-400">
                          {p.hours_until_start ? formatHoursUntil(p.hours_until_start) : '-'}
                        </div>
                      </>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
