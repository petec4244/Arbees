import { useQuery } from '@tanstack/react-query'
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, Cell } from 'recharts'

interface PnLBreakdownProps {
  days?: number
  type?: 'sport' | 'signal'
}

export function PnLBreakdownBySport({ days = 30 }: { days?: number }) {
  const { data: breakdown, isLoading } = useQuery({
    queryKey: ['performanceBreakdown', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/performance/breakdown?days=${days}`)
      return res.json()
    },
    refetchInterval: 30000,
  })

  if (isLoading) {
    return (
      <div className="bg-gray-800 rounded-lg p-4 h-64 flex items-center justify-center">
        <span className="text-gray-400">Loading...</span>
      </div>
    )
  }

  const sportData = breakdown?.by_sport || {}
  const chartData = Object.entries(sportData)
    .map(([sport, data]: [string, any]) => ({
      sport: sport.toUpperCase(),
      pnl: data.pnl,
      trades: data.trades,
      win_rate: data.win_rate,
    }))
    .sort((a, b) => b.pnl - a.pnl)

  if (chartData.length === 0) {
    return (
      <div className="bg-gray-800 rounded-lg p-4">
        <h3 className="text-lg font-semibold mb-3">P&L by Sport</h3>
        <div className="h-48 flex items-center justify-center text-gray-500">
          No trade data available
        </div>
      </div>
    )
  }

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <h3 className="text-lg font-semibold mb-3">P&L by Sport</h3>
      <div className="space-y-2">
        {chartData.map((item) => (
          <div key={item.sport} className="flex items-center gap-3">
            <span className="w-12 text-xs text-gray-400 font-mono">{item.sport}</span>
            <div className="flex-1 h-6 bg-gray-700 rounded relative overflow-hidden">
              <div
                className={`absolute top-0 bottom-0 ${item.pnl >= 0 ? 'bg-green-600' : 'bg-red-600'} transition-all`}
                style={{
                  width: `${Math.min(Math.abs(item.pnl) / Math.max(...chartData.map(d => Math.abs(d.pnl))) * 100, 100)}%`,
                  left: item.pnl >= 0 ? '0' : undefined,
                  right: item.pnl < 0 ? '0' : undefined,
                }}
              />
              <span className="absolute inset-0 flex items-center px-2 text-xs font-mono text-white">
                {item.pnl >= 0 ? '+' : ''}${item.pnl.toFixed(2)}
              </span>
            </div>
            <span className="w-20 text-xs text-gray-400 text-right">
              {item.trades} trades
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}

export function PnLBreakdownBySignalType({ days = 30 }: { days?: number }) {
  const { data: breakdown, isLoading } = useQuery({
    queryKey: ['performanceBreakdown', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/performance/breakdown?days=${days}`)
      return res.json()
    },
    refetchInterval: 30000,
  })

  if (isLoading) {
    return (
      <div className="bg-gray-800 rounded-lg p-4 h-64 flex items-center justify-center">
        <span className="text-gray-400">Loading...</span>
      </div>
    )
  }

  const signalData = breakdown?.by_signal_type || {}
  const chartData = Object.entries(signalData)
    .map(([type, data]: [string, any]) => ({
      type: formatSignalType(type),
      rawType: type,
      pnl: data.pnl,
      trades: data.trades,
      win_rate: data.win_rate,
      avg_edge: data.avg_edge,
    }))
    .sort((a, b) => b.pnl - a.pnl)

  if (chartData.length === 0) {
    return (
      <div className="bg-gray-800 rounded-lg p-4">
        <h3 className="text-lg font-semibold mb-3">P&L by Signal Type</h3>
        <div className="h-48 flex items-center justify-center text-gray-500">
          No trade data available
        </div>
      </div>
    )
  }

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <h3 className="text-lg font-semibold mb-3">P&L by Signal Type</h3>
      <div className="space-y-4">
        {chartData.map((item) => (
          <div key={item.rawType} className="border-b border-gray-700 pb-3 last:border-0">
            <div className="flex justify-between items-center mb-1">
              <span className="text-sm font-medium">{item.type}</span>
              <span className={`text-lg font-mono ${item.pnl >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                {item.pnl >= 0 ? '+' : ''}${item.pnl.toFixed(2)}
              </span>
            </div>
            <div className="flex gap-4 text-xs text-gray-400">
              <span>{item.win_rate.toFixed(0)}% win</span>
              <span>{item.trades} trades</span>
              {item.avg_edge > 0 && <span>Avg edge: {item.avg_edge.toFixed(1)}%</span>}
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}

function formatSignalType(type: string): string {
  const mapping: Record<string, string> = {
    'model_edge_yes': 'Model Edge (Yes)',
    'model_edge_no': 'Model Edge (No)',
    'cross_market_arb': 'Arbitrage',
    'cross_market_arb_no': 'Arbitrage (No)',
    'mean_reversion': 'Mean Reversion',
    'lagging_market': 'Lagging Market',
    'unknown': 'Other',
  }
  return mapping[type.toLowerCase()] || type.replace(/_/g, ' ').replace(/\b\w/g, l => l.toUpperCase())
}

export default function PnLBreakdown({ days = 30 }: { days?: number }) {
  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
      <PnLBreakdownBySport days={days} />
      <PnLBreakdownBySignalType days={days} />
    </div>
  )
}
