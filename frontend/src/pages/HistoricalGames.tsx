import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import {
  TrendingUp,
  TrendingDown,
  Calendar,
  Filter,
  ChevronLeft,
  ChevronRight,
  X,
  Activity,
  Target,
  Clock,
  BarChart3,
} from 'lucide-react'

// Types
interface ArchivedGame {
  archive_id: number
  game_id: string
  sport: string
  home_team: string
  away_team: string
  final_home_score: number
  final_away_score: number
  ended_at: string
  archived_at: string
  total_trades: number
  winning_trades: number
  losing_trades: number
  total_pnl: number
  win_rate: number
  capture_rate: number
}

interface ArchivedTrade {
  trade_id: string
  signal_type: string | null
  platform: string
  market_type: string | null
  side: string
  entry_price: number
  exit_price: number | null
  size: number
  opened_at: string
  closed_at: string | null
  outcome: string | null
  pnl: number | null
  edge_at_entry: number | null
}

interface ArchivedSignal {
  signal_id: string
  signal_type: string
  direction: string
  team: string | null
  model_prob: number | null
  market_prob: number | null
  edge_pct: number
  generated_at: string
  was_executed: boolean
}

interface GameDetail extends ArchivedGame {
  trades: ArchivedTrade[]
  signals: ArchivedSignal[]
}

interface HistoricalSummary {
  total_games: number
  total_trades: number
  total_pnl: number
  overall_win_rate: number
  total_wins: number
  total_losses: number
}

interface Filters {
  sport: string
  outcome: string
  fromDate: string
  toDate: string
  sortBy: string
  sortOrder: string
}

// Sport color map
const sportColors: Record<string, string> = {
  nba: 'bg-orange-900/50 text-orange-300',
  nfl: 'bg-green-900/50 text-green-300',
  nhl: 'bg-blue-900/50 text-blue-300',
  mlb: 'bg-red-900/50 text-red-300',
  ncaaf: 'bg-purple-900/50 text-purple-300',
  ncaab: 'bg-yellow-900/50 text-yellow-300',
  soccer: 'bg-emerald-900/50 text-emerald-300',
  mls: 'bg-teal-900/50 text-teal-300',
  default: 'bg-gray-700 text-gray-300',
}

export default function HistoricalGames() {
  const [page, setPage] = useState(1)
  const [pageSize] = useState(20)
  const [showFilters, setShowFilters] = useState(false)
  const [selectedGame, setSelectedGame] = useState<string | null>(null)
  const [filters, setFilters] = useState<Filters>({
    sport: '',
    outcome: '',
    fromDate: '',
    toDate: '',
    sortBy: 'ended_at',
    sortOrder: 'desc',
  })

  // Build query string
  const buildQueryString = () => {
    const params = new URLSearchParams()
    params.set('page', page.toString())
    params.set('page_size', pageSize.toString())
    if (filters.sport) params.set('sport', filters.sport)
    if (filters.outcome) params.set('outcome', filters.outcome)
    if (filters.fromDate) params.set('from_date', filters.fromDate)
    if (filters.toDate) params.set('to_date', filters.toDate)
    params.set('sort_by', filters.sortBy)
    params.set('sort_order', filters.sortOrder)
    return params.toString()
  }

  // Fetch historical games
  const { data: gamesData, isLoading } = useQuery({
    queryKey: ['historical-games', page, filters],
    queryFn: async () => {
      const res = await fetch(`/api/historical/games?${buildQueryString()}`)
      return res.json()
    },
  })

  // Fetch summary
  const { data: summary } = useQuery<HistoricalSummary>({
    queryKey: ['historical-summary', filters.fromDate, filters.toDate],
    queryFn: async () => {
      const params = new URLSearchParams()
      if (filters.fromDate) params.set('from_date', filters.fromDate)
      if (filters.toDate) params.set('to_date', filters.toDate)
      const res = await fetch(`/api/historical/summary?${params.toString()}`)
      return res.json()
    },
  })

  // Fetch game detail when selected
  const { data: gameDetail, isLoading: detailLoading } = useQuery<GameDetail>({
    queryKey: ['historical-game-detail', selectedGame],
    queryFn: async () => {
      const res = await fetch(`/api/historical/games/${selectedGame}`)
      return res.json()
    },
    enabled: !!selectedGame,
  })

  const games: ArchivedGame[] = gamesData?.games || []
  const totalPages = Math.ceil((gamesData?.total || 0) / pageSize)

  const handleFilterChange = (key: keyof Filters, value: string) => {
    setFilters(prev => ({ ...prev, [key]: value }))
    setPage(1) // Reset to first page on filter change
  }

  const clearFilters = () => {
    setFilters({
      sport: '',
      outcome: '',
      fromDate: '',
      toDate: '',
      sortBy: 'ended_at',
      sortOrder: 'desc',
    })
    setPage(1)
  }

  const getSportClass = (sport: string) => {
    return sportColors[sport.toLowerCase()] || sportColors.default
  }

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4">
        <div>
          <h1 className="text-3xl font-bold">Historical Games</h1>
          <p className="text-gray-400 mt-1">Archived games and performance analysis</p>
        </div>
        <button
          onClick={() => setShowFilters(!showFilters)}
          className="flex items-center gap-2 px-4 py-2 bg-gray-700 hover:bg-gray-600 rounded-lg transition-colors"
        >
          <Filter className="w-4 h-4" />
          <span>Filters</span>
          {(filters.sport || filters.outcome || filters.fromDate || filters.toDate) && (
            <span className="w-2 h-2 bg-blue-400 rounded-full" />
          )}
        </button>
      </div>

      {/* Summary Cards */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <SummaryCard
          icon={<BarChart3 className="w-5 h-5" />}
          title="Total Games"
          value={summary?.total_games || 0}
        />
        <SummaryCard
          icon={<Activity className="w-5 h-5" />}
          title="Total Trades"
          value={summary?.total_trades || 0}
          subtitle={`${summary?.total_wins || 0}W / ${summary?.total_losses || 0}L`}
        />
        <SummaryCard
          icon={<Target className="w-5 h-5 text-blue-400" />}
          title="Win Rate"
          value={`${((summary?.overall_win_rate || 0) * 100).toFixed(1)}%`}
          className="text-blue-400"
        />
        <SummaryCard
          icon={(summary?.total_pnl || 0) >= 0
            ? <TrendingUp className="w-5 h-5 text-green-400" />
            : <TrendingDown className="w-5 h-5 text-red-400" />
          }
          title="Total P&L"
          value={`$${(summary?.total_pnl || 0).toFixed(2)}`}
          className={(summary?.total_pnl || 0) >= 0 ? 'text-green-400' : 'text-red-400'}
        />
      </div>

      {/* Filters Panel */}
      {showFilters && (
        <div className="bg-gray-800 rounded-lg p-4 space-y-4">
          <div className="flex justify-between items-center">
            <h3 className="font-medium">Filters</h3>
            <button
              onClick={clearFilters}
              className="text-sm text-gray-400 hover:text-white"
            >
              Clear all
            </button>
          </div>
          <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-4 gap-4">
            {/* Sport Filter */}
            <div>
              <label className="block text-sm text-gray-400 mb-1">Sport</label>
              <select
                value={filters.sport}
                onChange={(e) => handleFilterChange('sport', e.target.value)}
                className="w-full bg-gray-700 border border-gray-600 rounded-lg px-3 py-2 text-sm focus:outline-none focus:border-blue-500"
              >
                <option value="">All Sports</option>
                <option value="nba">NBA</option>
                <option value="nfl">NFL</option>
                <option value="nhl">NHL</option>
                <option value="mlb">MLB</option>
                <option value="ncaaf">NCAAF</option>
                <option value="ncaab">NCAAB</option>
                <option value="soccer">Soccer</option>
                <option value="mls">MLS</option>
              </select>
            </div>

            {/* Outcome Filter */}
            <div>
              <label className="block text-sm text-gray-400 mb-1">Outcome</label>
              <select
                value={filters.outcome}
                onChange={(e) => handleFilterChange('outcome', e.target.value)}
                className="w-full bg-gray-700 border border-gray-600 rounded-lg px-3 py-2 text-sm focus:outline-none focus:border-blue-500"
              >
                <option value="">All Outcomes</option>
                <option value="profitable">Profitable</option>
                <option value="loss">Loss</option>
                <option value="breakeven">Breakeven</option>
              </select>
            </div>

            {/* Date Range */}
            <div>
              <label className="block text-sm text-gray-400 mb-1">From Date</label>
              <input
                type="date"
                value={filters.fromDate}
                onChange={(e) => handleFilterChange('fromDate', e.target.value)}
                className="w-full bg-gray-700 border border-gray-600 rounded-lg px-3 py-2 text-sm focus:outline-none focus:border-blue-500"
              />
            </div>
            <div>
              <label className="block text-sm text-gray-400 mb-1">To Date</label>
              <input
                type="date"
                value={filters.toDate}
                onChange={(e) => handleFilterChange('toDate', e.target.value)}
                className="w-full bg-gray-700 border border-gray-600 rounded-lg px-3 py-2 text-sm focus:outline-none focus:border-blue-500"
              />
            </div>
          </div>

          {/* Sort Options */}
          <div className="flex gap-4 pt-2 border-t border-gray-700">
            <div className="flex items-center gap-2">
              <label className="text-sm text-gray-400">Sort by:</label>
              <select
                value={filters.sortBy}
                onChange={(e) => handleFilterChange('sortBy', e.target.value)}
                className="bg-gray-700 border border-gray-600 rounded px-2 py-1 text-sm"
              >
                <option value="ended_at">Date</option>
                <option value="total_pnl">P&L</option>
                <option value="win_rate">Win Rate</option>
                <option value="total_trades">Trades</option>
              </select>
            </div>
            <div className="flex items-center gap-2">
              <label className="text-sm text-gray-400">Order:</label>
              <select
                value={filters.sortOrder}
                onChange={(e) => handleFilterChange('sortOrder', e.target.value)}
                className="bg-gray-700 border border-gray-600 rounded px-2 py-1 text-sm"
              >
                <option value="desc">Descending</option>
                <option value="asc">Ascending</option>
              </select>
            </div>
          </div>
        </div>
      )}

      {/* Games Table */}
      <div className="bg-gray-800 rounded-lg overflow-hidden">
        <div className="overflow-x-auto">
          <table className="min-w-full divide-y divide-gray-700">
            <thead className="bg-gray-700">
              <tr>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Date</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Sport</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Game</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Score</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Trades</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Win Rate</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">P&L</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-700">
              {isLoading ? (
                <tr>
                  <td colSpan={7} className="px-6 py-8 text-center text-gray-400">
                    Loading...
                  </td>
                </tr>
              ) : games.length === 0 ? (
                <tr>
                  <td colSpan={7} className="px-6 py-8 text-center text-gray-400">
                    No archived games found
                  </td>
                </tr>
              ) : (
                games.map((game) => (
                  <tr
                    key={game.archive_id}
                    onClick={() => setSelectedGame(game.game_id)}
                    className="hover:bg-gray-700/50 cursor-pointer transition-colors"
                  >
                    <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-400">
                      <div className="flex items-center gap-2">
                        <Calendar className="w-4 h-4" />
                        {new Date(game.ended_at).toLocaleDateString()}
                      </div>
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap">
                      <span className={`px-2 py-1 rounded text-xs font-medium uppercase ${getSportClass(game.sport)}`}>
                        {game.sport}
                      </span>
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap text-sm">
                      <div className="font-medium">
                        {game.away_team} @ {game.home_team}
                      </div>
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap text-sm font-mono">
                      {game.final_away_score} - {game.final_home_score}
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap text-sm">
                      <span className="font-mono">{game.total_trades}</span>
                      <span className="text-gray-500 ml-1">
                        ({game.winning_trades}W/{game.losing_trades}L)
                      </span>
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap">
                      <div className="flex items-center gap-2">
                        <div className="w-16 h-2 bg-gray-700 rounded-full overflow-hidden">
                          <div
                            className={`h-full rounded-full ${
                              game.win_rate >= 0.6 ? 'bg-green-500' :
                              game.win_rate >= 0.4 ? 'bg-yellow-500' : 'bg-red-500'
                            }`}
                            style={{ width: `${game.win_rate * 100}%` }}
                          />
                        </div>
                        <span className="text-sm font-mono">
                          {(game.win_rate * 100).toFixed(0)}%
                        </span>
                      </div>
                    </td>
                    <td className="px-4 py-3 whitespace-nowrap">
                      <span className={`font-mono font-medium ${
                        game.total_pnl >= 0 ? 'text-green-400' : 'text-red-400'
                      }`}>
                        {game.total_pnl >= 0 ? '+' : ''}${game.total_pnl.toFixed(2)}
                      </span>
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>

        {/* Pagination */}
        {totalPages > 1 && (
          <div className="flex items-center justify-between px-4 py-3 border-t border-gray-700">
            <div className="text-sm text-gray-400">
              Page {page} of {totalPages} ({gamesData?.total || 0} games)
            </div>
            <div className="flex gap-2">
              <button
                onClick={() => setPage(p => Math.max(1, p - 1))}
                disabled={page === 1}
                className="p-2 bg-gray-700 hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed rounded"
              >
                <ChevronLeft className="w-4 h-4" />
              </button>
              <button
                onClick={() => setPage(p => Math.min(totalPages, p + 1))}
                disabled={page === totalPages}
                className="p-2 bg-gray-700 hover:bg-gray-600 disabled:opacity-50 disabled:cursor-not-allowed rounded"
              >
                <ChevronRight className="w-4 h-4" />
              </button>
            </div>
          </div>
        )}
      </div>

      {/* Game Detail Modal */}
      {selectedGame && (
        <GameDetailModal
          game={gameDetail}
          isLoading={detailLoading}
          onClose={() => setSelectedGame(null)}
        />
      )}
    </div>
  )
}

function SummaryCard({
  icon,
  title,
  value,
  subtitle,
  className,
}: {
  icon?: React.ReactNode
  title: string
  value: string | number
  subtitle?: string
  className?: string
}) {
  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <div className="flex items-center space-x-2 text-gray-400 text-sm mb-1">
        {icon}
        <span>{title}</span>
      </div>
      <p className={`text-2xl font-bold ${className || ''}`}>{value}</p>
      {subtitle && <p className="text-xs text-gray-500 mt-1">{subtitle}</p>}
    </div>
  )
}

function GameDetailModal({
  game,
  isLoading,
  onClose,
}: {
  game: GameDetail | undefined
  isLoading: boolean
  onClose: () => void
}) {
  const [activeTab, setActiveTab] = useState<'trades' | 'signals'>('trades')

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/50">
      <div className="bg-gray-800 rounded-lg w-full max-w-4xl max-h-[90vh] overflow-hidden flex flex-col">
        {/* Header */}
        <div className="flex justify-between items-center p-4 border-b border-gray-700">
          <div>
            {isLoading ? (
              <div className="h-6 w-48 bg-gray-700 animate-pulse rounded" />
            ) : game ? (
              <>
                <h2 className="text-xl font-bold">
                  {game.away_team} @ {game.home_team}
                </h2>
                <p className="text-sm text-gray-400">
                  {new Date(game.ended_at).toLocaleDateString()} | Final: {game.final_away_score} - {game.final_home_score}
                </p>
              </>
            ) : (
              <p className="text-gray-400">Game not found</p>
            )}
          </div>
          <button
            onClick={onClose}
            className="p-2 hover:bg-gray-700 rounded-lg transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {isLoading ? (
          <div className="p-8 text-center text-gray-400">Loading...</div>
        ) : game ? (
          <>
            {/* Summary Stats */}
            <div className="grid grid-cols-4 gap-4 p-4 border-b border-gray-700">
              <div className="text-center">
                <p className="text-2xl font-bold">{game.total_trades}</p>
                <p className="text-xs text-gray-400">Trades</p>
              </div>
              <div className="text-center">
                <p className="text-2xl font-bold text-green-400">{game.winning_trades}</p>
                <p className="text-xs text-gray-400">Wins</p>
              </div>
              <div className="text-center">
                <p className="text-2xl font-bold text-red-400">{game.losing_trades}</p>
                <p className="text-xs text-gray-400">Losses</p>
              </div>
              <div className="text-center">
                <p className={`text-2xl font-bold ${game.total_pnl >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                  ${game.total_pnl.toFixed(2)}
                </p>
                <p className="text-xs text-gray-400">P&L</p>
              </div>
            </div>

            {/* Tabs */}
            <div className="flex border-b border-gray-700">
              <button
                onClick={() => setActiveTab('trades')}
                className={`px-4 py-2 text-sm font-medium transition-colors ${
                  activeTab === 'trades'
                    ? 'text-white border-b-2 border-blue-500'
                    : 'text-gray-400 hover:text-white'
                }`}
              >
                Trades ({game.trades.length})
              </button>
              <button
                onClick={() => setActiveTab('signals')}
                className={`px-4 py-2 text-sm font-medium transition-colors ${
                  activeTab === 'signals'
                    ? 'text-white border-b-2 border-blue-500'
                    : 'text-gray-400 hover:text-white'
                }`}
              >
                Signals ({game.signals.length})
              </button>
            </div>

            {/* Tab Content */}
            <div className="flex-1 overflow-y-auto">
              {activeTab === 'trades' ? (
                <TradesTable trades={game.trades} />
              ) : (
                <SignalsTable signals={game.signals} />
              )}
            </div>
          </>
        ) : null}
      </div>
    </div>
  )
}

function TradesTable({ trades }: { trades: ArchivedTrade[] }) {
  if (trades.length === 0) {
    return (
      <div className="p-8 text-center text-gray-400">
        No trades for this game
      </div>
    )
  }

  return (
    <table className="min-w-full divide-y divide-gray-700">
      <thead className="bg-gray-700 sticky top-0">
        <tr>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Time</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Type</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Side</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Size</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Entry</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Exit</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Edge</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">P&L</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-gray-700">
        {trades.map((trade) => (
          <tr key={trade.trade_id} className="hover:bg-gray-700/50">
            <td className="px-4 py-2 whitespace-nowrap text-xs text-gray-400">
              {new Date(trade.opened_at).toLocaleTimeString()}
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs">
              {trade.signal_type || '-'}
            </td>
            <td className="px-4 py-2 whitespace-nowrap">
              <span className={`px-2 py-0.5 rounded text-xs font-medium ${
                trade.side === 'buy'
                  ? 'bg-green-900/50 text-green-300'
                  : 'bg-red-900/50 text-red-300'
              }`}>
                {trade.side === 'buy' ? 'YES WIN' : 'NO LOSE'}
              </span>
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono">
              ${trade.size.toFixed(2)}
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono">
              {(trade.entry_price * 100).toFixed(1)}%
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono">
              {trade.exit_price ? `${(trade.exit_price * 100).toFixed(1)}%` : '-'}
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono text-blue-400">
              {trade.edge_at_entry ? `${trade.edge_at_entry.toFixed(1)}%` : '-'}
            </td>
            <td className="px-4 py-2 whitespace-nowrap">
              {trade.pnl !== null ? (
                <span className={`text-xs font-mono font-medium ${
                  trade.pnl >= 0 ? 'text-green-400' : 'text-red-400'
                }`}>
                  {trade.pnl >= 0 ? '+' : ''}${trade.pnl.toFixed(2)}
                </span>
              ) : (
                <span className="text-gray-500 text-xs">-</span>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function SignalsTable({ signals }: { signals: ArchivedSignal[] }) {
  if (signals.length === 0) {
    return (
      <div className="p-8 text-center text-gray-400">
        No signals for this game
      </div>
    )
  }

  return (
    <table className="min-w-full divide-y divide-gray-700">
      <thead className="bg-gray-700 sticky top-0">
        <tr>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Time</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Type</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Direction</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Model</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Market</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Edge</th>
          <th className="px-4 py-2 text-left text-xs font-medium text-gray-300 uppercase">Executed</th>
        </tr>
      </thead>
      <tbody className="divide-y divide-gray-700">
        {signals.map((signal) => (
          <tr key={signal.signal_id} className="hover:bg-gray-700/50">
            <td className="px-4 py-2 whitespace-nowrap text-xs text-gray-400">
              {new Date(signal.generated_at).toLocaleTimeString()}
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs">
              {signal.signal_type}
            </td>
            <td className="px-4 py-2 whitespace-nowrap">
              <span className={`px-2 py-0.5 rounded text-xs font-medium ${
                signal.direction === 'buy'
                  ? 'bg-blue-900/50 text-blue-300'
                  : signal.direction === 'sell'
                  ? 'bg-orange-900/50 text-orange-300'
                  : 'bg-gray-700 text-gray-300'
              }`}>
                {signal.direction.toUpperCase()}
              </span>
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono">
              {signal.model_prob ? `${(signal.model_prob * 100).toFixed(1)}%` : '-'}
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono">
              {signal.market_prob ? `${(signal.market_prob * 100).toFixed(1)}%` : '-'}
            </td>
            <td className="px-4 py-2 whitespace-nowrap text-xs font-mono text-blue-400">
              {signal.edge_pct.toFixed(1)}%
            </td>
            <td className="px-4 py-2 whitespace-nowrap">
              <span className={`px-2 py-0.5 rounded text-xs font-medium ${
                signal.was_executed
                  ? 'bg-green-900/50 text-green-300'
                  : 'bg-gray-700 text-gray-400'
              }`}>
                {signal.was_executed ? 'YES' : 'NO'}
              </span>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
