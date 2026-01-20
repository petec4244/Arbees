import { useQuery } from '@tanstack/react-query'
import { CheckCircle, XCircle, AlertCircle } from 'lucide-react'

interface TradeStatsTableProps {
  periods?: ('today' | '7d' | '30d' | 'all')[]
  showTargets?: boolean
}

const TARGETS = {
  win_rate: { min: 55, label: '>55%' },
  avg_edge: { min: 2.0, label: '>2.0%' },
  avg_pnl: { min: 1.50, label: '>$1.50' },
  max_loss: { max: 50, label: '<$50' },
}

export default function TradeStatsTable({
  periods = ['today', '7d', '30d', 'all'],
  showTargets = true,
}: TradeStatsTableProps) {
  // Fetch data for each period
  const { data: todayData } = useQuery({
    queryKey: ['performance', 1],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/performance?days=1')
      return res.json()
    },
    refetchInterval: 30000,
    enabled: periods.includes('today'),
  })

  const { data: week7Data } = useQuery({
    queryKey: ['performance', 7],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/performance?days=7')
      return res.json()
    },
    refetchInterval: 30000,
    enabled: periods.includes('7d'),
  })

  const { data: month30Data } = useQuery({
    queryKey: ['performance', 30],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/performance?days=30')
      return res.json()
    },
    refetchInterval: 30000,
    enabled: periods.includes('30d'),
  })

  const { data: allData } = useQuery({
    queryKey: ['performance', 365],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/performance?days=365')
      return res.json()
    },
    refetchInterval: 30000,
    enabled: periods.includes('all'),
  })

  // Also get max loss from trades
  const { data: tradesData } = useQuery({
    queryKey: ['trades', 'for-stats'],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/trades?limit=200')
      return res.json()
    },
    refetchInterval: 30000,
  })

  const getMaxLoss = (trades: any[], days?: number) => {
    if (!trades) return 0
    const cutoff = days ? new Date(Date.now() - days * 24 * 60 * 60 * 1000) : new Date(0)
    const filtered = trades.filter(
      (t: any) => new Date(t.entry_time) >= cutoff && t.pnl !== null && t.pnl < 0
    )
    return filtered.length > 0 ? Math.abs(Math.min(...filtered.map((t: any) => t.pnl))) : 0
  }

  const periodData: Record<string, any> = {
    today: todayData,
    '7d': week7Data,
    '30d': month30Data,
    all: allData,
  }

  const periodDays: Record<string, number | undefined> = {
    today: 1,
    '7d': 7,
    '30d': 30,
    all: undefined,
  }

  const stats = [
    {
      label: 'Trades',
      key: 'total_trades',
      format: (v: number) => v.toString(),
      target: null,
    },
    {
      label: 'Win Rate',
      key: 'win_rate',
      format: (v: number) => `${v.toFixed(1)}%`,
      target: TARGETS.win_rate,
    },
    {
      label: 'Avg P&L',
      key: 'avg_pnl',
      format: (v: number) => `$${v.toFixed(2)}`,
      target: TARGETS.avg_pnl,
    },
    {
      label: 'Total P&L',
      key: 'total_pnl',
      format: (v: number) => `$${v.toFixed(2)}`,
      target: null,
    },
    {
      label: 'Max Loss',
      key: 'max_loss',
      getValue: (data: any, days?: number) => getMaxLoss(tradesData, days),
      format: (v: number) => `$${v.toFixed(2)}`,
      target: TARGETS.max_loss,
    },
  ]

  const checkTarget = (stat: any, value: number) => {
    if (!stat.target) return null
    if ('min' in stat.target) {
      return value >= stat.target.min
    }
    if ('max' in stat.target) {
      return value <= stat.target.max
    }
    return null
  }

  return (
    <div className="bg-gray-800 rounded-lg overflow-hidden">
      <div className="p-4 border-b border-gray-700">
        <h3 className="text-lg font-semibold">Trade Statistics</h3>
      </div>
      <div className="overflow-x-auto">
        <table className="min-w-full">
          <thead className="bg-gray-700/50">
            <tr>
              <th className="px-4 py-3 text-left text-xs font-medium text-gray-400 uppercase">
                Metric
              </th>
              {periods.map((period) => (
                <th
                  key={period}
                  className="px-4 py-3 text-right text-xs font-medium text-gray-400 uppercase"
                >
                  {period === 'today' ? 'Today' : period.toUpperCase()}
                </th>
              ))}
              {showTargets && (
                <th className="px-4 py-3 text-right text-xs font-medium text-gray-400 uppercase">
                  Target
                </th>
              )}
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-700">
            {stats.map((stat) => (
              <tr key={stat.key} className="hover:bg-gray-700/30">
                <td className="px-4 py-3 text-sm text-gray-300">{stat.label}</td>
                {periods.map((period) => {
                  const data = periodData[period]
                  const days = periodDays[period]
                  const value = stat.getValue
                    ? stat.getValue(data, days)
                    : data?.[stat.key] ?? 0
                  const meetsTarget = checkTarget(stat, value)

                  return (
                    <td key={period} className="px-4 py-3 text-right">
                      <div className="flex items-center justify-end gap-2">
                        <span
                          className={`text-sm font-mono ${
                            stat.key === 'total_pnl' || stat.key === 'avg_pnl'
                              ? value >= 0
                                ? 'text-green-400'
                                : 'text-red-400'
                              : 'text-white'
                          }`}
                        >
                          {stat.format(value)}
                        </span>
                        {meetsTarget !== null && (
                          <span>
                            {meetsTarget ? (
                              <CheckCircle className="w-4 h-4 text-green-400" />
                            ) : (
                              <AlertCircle className="w-4 h-4 text-yellow-400" />
                            )}
                          </span>
                        )}
                      </div>
                    </td>
                  )
                })}
                {showTargets && (
                  <td className="px-4 py-3 text-right text-sm text-gray-500">
                    {stat.target?.label || '-'}
                  </td>
                )}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}
