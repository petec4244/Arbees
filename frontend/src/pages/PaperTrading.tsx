import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import {
  TrendingUp,
  TrendingDown,
  DollarSign,
  Target,
  Clock,
  Activity,
  AlertTriangle,
  CheckCircle,
  Zap,
  Shield,
  ChevronDown,
  ChevronUp,
} from 'lucide-react'
import { ExposureBySport, ExposureByGame } from '../components/ExposureGauge'
import LatencyChart from '../components/LatencyChart'
import { useUIPreferences } from '../hooks/useUIPreferences'

export default function PaperTrading() {
  const { riskDisplayMode, setRiskDisplayMode, showLatencyChart, setShowLatencyChart } = useUIPreferences()
  const [showRiskSection, setShowRiskSection] = useState(true)

  const { data: status, isLoading: statusLoading } = useQuery({
    queryKey: ['paper-status'],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/status')
      return res.json()
    },
    refetchInterval: 5000,
  })

  const { data: performance } = useQuery({
    queryKey: ['performance'],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/performance')
      return res.json()
    },
    refetchInterval: 10000,
  })

  const { data: riskMetrics } = useQuery({
    queryKey: ['riskMetrics'],
    queryFn: async () => {
      const res = await fetch('/api/risk/metrics')
      return res.json()
    },
    refetchInterval: 5000,
  })

  const { data: trades } = useQuery({
    queryKey: ['trades'],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/trades?limit=50')
      return res.json()
    },
    refetchInterval: 5000,
  })

  const { data: riskEvents } = useQuery({
    queryKey: ['riskEvents'],
    queryFn: async () => {
      const res = await fetch('/api/risk/events?limit=20')
      return res.json()
    },
    refetchInterval: 10000,
  })

  const bankroll = status?.bankroll || {}
  const openPositions = status?.open_positions || []

  return (
    <div className="space-y-6">
      <div className="flex flex-col sm:flex-row sm:justify-between sm:items-center gap-4">
        <div>
          <h1 className="text-3xl font-bold">Paper Trading</h1>
          <p className="text-gray-400 mt-1">Track performance and manage risk</p>
        </div>
        <div className="flex items-center space-x-2 text-sm text-gray-400">
          <Activity className="w-4 h-4" />
          <span>{status?.open_positions_count || 0} open positions</span>
        </div>
      </div>

      {/* Bankroll Summary */}
      <div className="grid grid-cols-2 md:grid-cols-5 gap-4">
        <StatCard
          icon={<DollarSign className="w-5 h-5" />}
          title="Current Bankroll"
          value={`$${(bankroll.current_balance || 1000).toFixed(2)}`}
          subtitle={`Initial: $${(bankroll.initial_balance || 1000).toFixed(2)}`}
        />
        <StatCard
          icon={<Target className="w-5 h-5 text-blue-400" />}
          title="Available"
          value={`$${(bankroll.available_balance || bankroll.current_balance || 1000).toFixed(2)}`}
          subtitle="For new trades"
          className="text-blue-400"
        />
        <StatCard
          icon={<Clock className="w-5 h-5 text-yellow-400" />}
          title="Reserved"
          value={`$${(bankroll.reserved_balance || 0).toFixed(2)}`}
          subtitle="In open positions"
          className="text-yellow-400"
        />
        <StatCard
          icon={(performance?.total_pnl || 0) >= 0 ? <TrendingUp className="w-5 h-5 text-green-400" /> : <TrendingDown className="w-5 h-5 text-red-400" />}
          title="Total P&L"
          value={`$${(performance?.total_pnl || 0).toFixed(2)}`}
          subtitle={`ROI: ${(performance?.roi_pct || 0).toFixed(1)}%`}
          className={(performance?.total_pnl || 0) >= 0 ? 'text-green-400' : 'text-red-400'}
        />
        <StatCard
          icon={<Activity className="w-5 h-5" />}
          title="Win Rate"
          value={`${(performance?.win_rate || 0).toFixed(1)}%`}
          subtitle={`${performance?.total_trades || 0} total trades`}
        />
      </div>

      {/* Risk Management Section */}
      <div className="bg-gray-800 rounded-lg overflow-hidden">
        <button
          onClick={() => setShowRiskSection(!showRiskSection)}
          className="w-full p-4 flex justify-between items-center hover:bg-gray-700/50 transition-colors"
        >
          <div className="flex items-center gap-3">
            <Shield className="w-5 h-5 text-yellow-400" />
            <h2 className="text-xl font-semibold">Risk Management</h2>
            {riskMetrics?.circuit_breaker_open && (
              <span className="px-2 py-1 bg-red-900/50 text-red-300 text-xs rounded animate-pulse">
                CIRCUIT BREAKER OPEN
              </span>
            )}
          </div>
          <div className="flex items-center gap-4">
            {/* Display Mode Toggle */}
            <div className="flex items-center gap-2 bg-gray-700 rounded-lg p-1">
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  setRiskDisplayMode('compact')
                }}
                className={`px-3 py-1 rounded text-xs transition-colors ${
                  riskDisplayMode === 'compact' ? 'bg-gray-600 text-white' : 'text-gray-400 hover:text-white'
                }`}
              >
                Compact
              </button>
              <button
                onClick={(e) => {
                  e.stopPropagation()
                  setRiskDisplayMode('detailed')
                }}
                className={`px-3 py-1 rounded text-xs transition-colors ${
                  riskDisplayMode === 'detailed' ? 'bg-gray-600 text-white' : 'text-gray-400 hover:text-white'
                }`}
              >
                Detailed
              </button>
            </div>
            {showRiskSection ? <ChevronUp className="w-5 h-5 text-gray-400" /> : <ChevronDown className="w-5 h-5 text-gray-400" />}
          </div>
        </button>

        {showRiskSection && (
          <div className="p-4 pt-0 space-y-4">
            {/* Risk KPIs */}
            <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
              <RiskKPICard
                icon={<DollarSign className="w-4 h-4" />}
                title="Daily P&L"
                value={`$${(riskMetrics?.daily_pnl || 0).toFixed(2)}`}
                progress={riskMetrics?.daily_limit_pct || 0}
                subtitle={`$${(riskMetrics?.daily_limit_remaining || 0).toFixed(2)} left`}
                positive={(riskMetrics?.daily_pnl || 0) >= 0}
              />
              <RiskKPICard
                icon={<Target className="w-4 h-4" />}
                title="Daily Limit"
                value={`${(riskMetrics?.daily_limit_pct || 0).toFixed(0)}%`}
                progress={riskMetrics?.daily_limit_pct || 0}
                subtitle={`of $${riskMetrics?.daily_limit || 100}`}
              />
              <RiskKPICard
                icon={riskMetrics?.circuit_breaker_open ? <AlertTriangle className="w-4 h-4 text-red-400" /> : <CheckCircle className="w-4 h-4 text-green-400" />}
                title="Circuit Breaker"
                value={riskMetrics?.circuit_breaker_open ? 'OPEN' : 'CLOSED'}
                subtitle={riskMetrics?.circuit_breaker_open ? 'Trading halted' : 'Normal ops'}
                status={riskMetrics?.circuit_breaker_open ? 'danger' : 'success'}
              />
              <RiskKPICard
                icon={<Zap className="w-4 h-4" />}
                title="Avg Latency"
                value={`${(riskMetrics?.avg_latency_ms || 0).toFixed(0)}ms`}
                subtitle={`P95: ${(riskMetrics?.p95_latency_ms || 0).toFixed(0)}ms`}
                status={
                  riskMetrics?.latency_status === 'critical' ? 'danger' :
                  riskMetrics?.latency_status === 'warning' ? 'warning' : 'success'
                }
              />
            </div>

            {/* Exposure Gauges */}
            {riskDisplayMode === 'compact' ? (
              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <ExposureBySport showHeader={true} />
                <ExposureByGame showHeader={true} limit={5} />
              </div>
            ) : (
              <div className="space-y-4">
                <ExposureBySport showHeader={true} />
                <ExposureByGame showHeader={true} limit={10} />
              </div>
            )}

            {/* Latency Chart Toggle */}
            <div className="flex items-center justify-between bg-gray-900/50 rounded p-3">
              <span className="text-sm text-gray-400">Show Latency Chart</span>
              <button
                onClick={() => setShowLatencyChart(!showLatencyChart)}
                className={`w-12 h-6 rounded-full transition-colors ${
                  showLatencyChart ? 'bg-green-600' : 'bg-gray-600'
                }`}
              >
                <div className={`w-5 h-5 bg-white rounded-full transition-transform transform ${
                  showLatencyChart ? 'translate-x-6' : 'translate-x-0.5'
                }`} />
              </button>
            </div>

            {/* Latency Chart */}
            {showLatencyChart && <LatencyChart height={180} showThresholds />}

            {/* Risk Events Log */}
            {riskDisplayMode === 'detailed' && riskEvents && riskEvents.length > 0 && (
              <div className="bg-gray-900/50 rounded-lg p-4">
                <h3 className="text-sm font-medium text-gray-400 mb-3">Recent Risk Events</h3>
                <div className="space-y-2 max-h-48 overflow-y-auto custom-scrollbar">
                  {riskEvents.slice(0, 10).map((event: any, idx: number) => (
                    <div
                      key={idx}
                      className="flex items-center gap-3 text-xs py-1 border-b border-gray-800 last:border-0"
                    >
                      <span className="text-gray-500 font-mono w-20">
                        {event.time ? new Date(event.time).toLocaleTimeString() : '--'}
                      </span>
                      <span className={`px-1.5 py-0.5 rounded ${
                        event.event_type === 'APPROVED'
                          ? 'bg-green-900/50 text-green-300'
                          : 'bg-red-900/50 text-red-300'
                      }`}>
                        {event.event_type}
                      </span>
                      <span className="text-gray-400">{event.reason}</span>
                      <span className="text-gray-300 flex-1 truncate">{event.message}</span>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* Open Positions */}
      {openPositions.length > 0 && (
        <div className="bg-gray-800 rounded-lg overflow-hidden">
          <h2 className="text-xl font-semibold p-4 border-b border-gray-700 flex items-center space-x-2">
            <Clock className="w-5 h-5 text-yellow-400" />
            <span>Open Positions ({openPositions.length})</span>
          </h2>
          <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4 p-4">
            {openPositions.map((pos: any, idx: number) => (
              <PositionCard key={idx} position={pos} />
            ))}
          </div>
        </div>
      )}

      {/* Trade History */}
      <div className="bg-gray-800 rounded-lg overflow-hidden">
        <h2 className="text-xl font-semibold p-4 border-b border-gray-700">Trade History</h2>
        <div className="overflow-x-auto">
          <table className="min-w-full divide-y divide-gray-700">
            <thead className="bg-gray-700">
              <tr>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Time</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Game</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Position</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Size</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Entry Price</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Exit Price</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">P&L</th>
                <th className="px-4 py-3 text-left text-xs font-medium text-gray-300 uppercase">Status</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-gray-700">
              {trades?.map((trade: any) => (
                <tr key={trade.trade_id} className="hover:bg-gray-700/50">
                  <td className="px-4 py-3 whitespace-nowrap text-sm text-gray-400">
                    {new Date(trade.entry_time).toLocaleString()}
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap text-sm">
                    <div className="flex items-center space-x-2">
                      <span className="px-1.5 py-0.5 rounded text-xs bg-gray-700 text-gray-300 uppercase font-medium">
                        {trade.sport}
                      </span>
                      <div>
                        <div className="font-medium">
                          {trade.away_team && trade.home_team ? (
                            <>
                              <span className={`${
                                trade.entry_price < 0.5
                                  ? 'text-orange-300 font-semibold'
                                  : 'text-orange-400/70'
                              }`}>
                                {trade.away_team}
                              </span>
                              <span className="text-gray-500"> @ </span>
                              <span className={`${
                                trade.entry_price >= 0.5
                                  ? 'text-blue-300 font-semibold'
                                  : 'text-blue-400/70'
                              }`}>
                                {trade.home_team}
                              </span>
                            </>
                          ) : (
                            `Game ${trade.game_id}`
                          )}
                        </div>
                        {trade.edge_at_entry && (
                          <div className="text-xs text-gray-400">
                            Edge: {trade.edge_at_entry.toFixed(1)}%
                          </div>
                        )}
                      </div>
                    </div>
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap">
                    <span className={`px-2 py-1 rounded text-xs font-medium ${
                      trade.side === 'buy'
                        ? 'bg-blue-900/50 text-blue-300 border border-blue-700'
                        : 'bg-orange-900/50 text-orange-300 border border-orange-700'
                    }`}>
                      {trade.side === 'buy' ? 'HOME' : 'AWAY'}
                    </span>
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap text-sm font-mono">
                    ${trade.size.toFixed(2)}
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap text-sm font-mono">
                    {(trade.entry_price * 100).toFixed(1)}%
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap text-sm font-mono">
                    {trade.exit_price ? `${(trade.exit_price * 100).toFixed(1)}%` : '-'}
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap">
                    {trade.pnl !== null ? (
                      <span className={`font-mono ${trade.pnl >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                        {trade.pnl >= 0 ? '+' : ''}${trade.pnl.toFixed(2)}
                      </span>
                    ) : (
                      <span className="text-gray-500">-</span>
                    )}
                  </td>
                  <td className="px-4 py-3 whitespace-nowrap">
                    <span className={`px-2 py-1 rounded text-xs font-medium ${
                      trade.status === 'closed'
                        ? (trade.outcome === 'win'
                            ? 'bg-green-900/50 text-green-300'
                            : 'bg-red-900/50 text-red-300')
                        : 'bg-yellow-900/50 text-yellow-300'
                    }`}>
                      {trade.status === 'closed' ? trade.outcome.toUpperCase() : 'OPEN'}
                    </span>
                  </td>
                </tr>
              ))}
              {(!trades || trades.length === 0) && (
                <tr>
                  <td colSpan={8} className="px-6 py-8 text-center text-gray-400">
                    No trades yet - signals will be executed automatically
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
    </div>
  )
}

function StatCard({
  icon,
  title,
  value,
  subtitle,
  className
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

function RiskKPICard({
  icon,
  title,
  value,
  progress,
  subtitle,
  positive,
  status,
}: {
  icon?: React.ReactNode
  title: string
  value: string
  progress?: number
  subtitle?: string
  positive?: boolean
  status?: 'success' | 'warning' | 'danger'
}) {
  const getProgressColor = () => {
    if (progress === undefined) return ''
    if (progress >= 90) return 'bg-red-500'
    if (progress >= 70) return 'bg-yellow-500'
    return 'bg-green-500'
  }

  const getStatusColor = () => {
    if (positive !== undefined) return positive ? 'text-green-400' : 'text-red-400'
    switch (status) {
      case 'success': return 'text-green-400'
      case 'warning': return 'text-yellow-400'
      case 'danger': return 'text-red-400'
      default: return 'text-white'
    }
  }

  return (
    <div className="bg-gray-900/50 rounded-lg p-3">
      <div className="flex items-center gap-2 text-gray-400 text-xs mb-1">
        {icon}
        <span>{title}</span>
      </div>
      <p className={`text-xl font-bold font-mono ${getStatusColor()}`}>{value}</p>
      {progress !== undefined && (
        <div className="mt-2">
          <div className="h-1.5 bg-gray-700 rounded-full overflow-hidden">
            <div
              className={`h-full ${getProgressColor()} transition-all`}
              style={{ width: `${Math.min(progress, 100)}%` }}
            />
          </div>
        </div>
      )}
      {subtitle && <p className="text-xs text-gray-500 mt-1">{subtitle}</p>}
    </div>
  )
}

function PositionCard({ position }: { position: any }) {
  const cost = position.side === 'buy'
    ? position.size * position.entry_price
    : position.size * (1 - position.entry_price)

  return (
    <div className="bg-gray-900/50 rounded-lg p-4 border border-gray-700">
      <div className="flex justify-between items-start mb-2">
        <div className="flex items-center space-x-2">
          <span className="px-1.5 py-0.5 rounded text-xs bg-gray-700 text-gray-300 uppercase font-medium">
            {position.sport}
          </span>
          <span className={`px-2 py-0.5 rounded text-xs font-medium ${
            position.side === 'buy'
              ? 'bg-blue-900/50 text-blue-300'
              : 'bg-orange-900/50 text-orange-300'
          }`}>
            {position.side === 'buy' ? 'HOME' : 'AWAY'}
          </span>
        </div>
        <span className="text-xs text-gray-500">
          {new Date(position.time).toLocaleTimeString()}
        </span>
      </div>
      <div className="font-medium mb-1">
        {position.away_team && position.home_team ? (
          <>
            <span className={`${
              position.entry_price < 0.5
                ? 'text-orange-300 font-semibold'
                : 'text-orange-400/70'
            }`}>
              {position.away_team}
            </span>
            <span className="text-gray-500"> @ </span>
            <span className={`${
              position.entry_price >= 0.5
                ? 'text-blue-300 font-semibold'
                : 'text-blue-400/70'
            }`}>
              {position.home_team}
            </span>
          </>
        ) : (
          `Game ${position.game_id}`
        )}
      </div>
      <div className="flex justify-between text-sm text-gray-400">
        <span>Size: <span className="text-white font-mono">${position.size.toFixed(2)}</span></span>
        <span>Entry: <span className="text-white font-mono">{(position.entry_price * 100).toFixed(1)}%</span></span>
      </div>
      <div className="mt-2 pt-2 border-t border-gray-700 text-sm">
        <span className="text-gray-400">Cost: </span>
        <span className="text-yellow-400 font-mono">${cost.toFixed(2)}</span>
      </div>
    </div>
  )
}
