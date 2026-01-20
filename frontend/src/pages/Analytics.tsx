import { useQuery } from '@tanstack/react-query'
import { TrendingUp, TrendingDown, Activity, Target, Calendar } from 'lucide-react'
import EquityCurve from '../components/EquityCurve'
import PnLBreakdown from '../components/PnLBreakdown'
import TradeStatsTable from '../components/TradeStatsTable'
import PnLHistogram from '../components/PnLHistogram'
import { useUIPreferences, useTimePeriodDays } from '../hooks/useUIPreferences'

export default function Analytics() {
  const { analyticsTimePeriod, setAnalyticsTimePeriod } = useUIPreferences()
  const days = useTimePeriodDays()

  const { data: performance } = useQuery({
    queryKey: ['performance', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/performance?days=${days}`)
      return res.json()
    },
    refetchInterval: 30000,
  })

  const { data: equityHistory } = useQuery({
    queryKey: ['equityHistory', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/equity-history?days=${days}`)
      return res.json()
    },
    refetchInterval: 30000,
  })

  // Calculate max drawdown from equity history
  const maxDrawdown = equityHistory
    ? Math.max(...equityHistory.map((d: any) => d.drawdown_pct || 0))
    : 0

  const periods: { label: string; value: '7d' | '30d' | '90d' | 'all' }[] = [
    { label: '7D', value: '7d' },
    { label: '30D', value: '30d' },
    { label: '90D', value: '90d' },
    { label: 'All', value: 'all' },
  ]

  return (
    <div className="space-y-6">
      {/* Header */}
      <div className="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-4">
        <div>
          <h1 className="text-3xl font-bold">Performance Analytics</h1>
          <p className="text-gray-400 mt-1">Track your trading performance and strategy effectiveness</p>
        </div>

        {/* Time Period Selector */}
        <div className="flex items-center gap-2 bg-gray-800 rounded-lg p-1">
          {periods.map((period) => (
            <button
              key={period.value}
              onClick={() => setAnalyticsTimePeriod(period.value)}
              className={`px-4 py-2 rounded-md text-sm font-medium transition-colors ${
                analyticsTimePeriod === period.value
                  ? 'bg-green-600 text-white'
                  : 'text-gray-400 hover:text-white hover:bg-gray-700'
              }`}
            >
              {period.label}
            </button>
          ))}
        </div>
      </div>

      {/* Summary KPIs */}
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <div className="bg-gray-800 rounded-lg p-4">
          <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
            {(performance?.total_pnl || 0) >= 0 ? (
              <TrendingUp className="w-4 h-4 text-green-400" />
            ) : (
              <TrendingDown className="w-4 h-4 text-red-400" />
            )}
            <span>Total P&L</span>
          </div>
          <p className={`text-2xl font-bold font-mono ${
            (performance?.total_pnl || 0) >= 0 ? 'text-green-400' : 'text-red-400'
          }`}>
            {(performance?.total_pnl || 0) >= 0 ? '+' : ''}${(performance?.total_pnl || 0).toFixed(2)}
          </p>
          <p className="text-xs text-gray-500 mt-1">
            ROI: {(performance?.roi_pct || 0).toFixed(1)}%
          </p>
        </div>

        <div className="bg-gray-800 rounded-lg p-4">
          <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
            <Target className="w-4 h-4 text-blue-400" />
            <span>Win Rate</span>
          </div>
          <p className="text-2xl font-bold font-mono text-white">
            {(performance?.win_rate || 0).toFixed(1)}%
          </p>
          <p className="text-xs text-gray-500 mt-1">
            {performance?.winning_trades || 0}/{performance?.total_trades || 0} trades
          </p>
        </div>

        <div className="bg-gray-800 rounded-lg p-4">
          <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
            <Activity className="w-4 h-4 text-yellow-400" />
            <span>Max Drawdown</span>
          </div>
          <p className="text-2xl font-bold font-mono text-yellow-400">
            -{maxDrawdown.toFixed(1)}%
          </p>
          <p className="text-xs text-gray-500 mt-1">From peak equity</p>
        </div>

        <div className="bg-gray-800 rounded-lg p-4">
          <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
            <Calendar className="w-4 h-4 text-purple-400" />
            <span>Avg P&L/Trade</span>
          </div>
          <p className={`text-2xl font-bold font-mono ${
            (performance?.avg_pnl || 0) >= 0 ? 'text-green-400' : 'text-red-400'
          }`}>
            ${(performance?.avg_pnl || 0).toFixed(2)}
          </p>
          <p className="text-xs text-gray-500 mt-1">Per closed trade</p>
        </div>
      </div>

      {/* Full Equity Curve */}
      <EquityCurve days={days} height={350} showDrawdown />

      {/* P&L Breakdown */}
      <PnLBreakdown days={days} />

      {/* Stats Table */}
      <TradeStatsTable showTargets />

      {/* P&L Distribution */}
      <PnLHistogram days={days} height={250} bins={12} />
    </div>
  )
}
