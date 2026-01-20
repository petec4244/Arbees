import { useQuery } from '@tanstack/react-query'
import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  Cell,
  ReferenceLine,
} from 'recharts'

interface PnLHistogramProps {
  days?: number
  height?: number
  bins?: number
}

export default function PnLHistogram({
  days = 30,
  height = 200,
  bins = 10,
}: PnLHistogramProps) {
  const { data: trades, isLoading } = useQuery({
    queryKey: ['trades', 'histogram', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/trades?limit=200`)
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

  // Filter trades by date and closed status
  const cutoff = new Date(Date.now() - days * 24 * 60 * 60 * 1000)
  const closedTrades = (trades || []).filter(
    (t: any) =>
      t.status === 'closed' &&
      t.pnl !== null &&
      new Date(t.entry_time) >= cutoff
  )

  if (closedTrades.length === 0) {
    return (
      <div className="bg-gray-800 rounded-lg p-4">
        <h3 className="text-lg font-semibold mb-3">P&L Distribution</h3>
        <div className="h-48 flex items-center justify-center text-gray-500">
          No closed trades in period
        </div>
      </div>
    )
  }

  // Calculate histogram bins
  const pnlValues = closedTrades.map((t: any) => t.pnl)
  const minPnl = Math.min(...pnlValues)
  const maxPnl = Math.max(...pnlValues)

  // Create symmetric bins around 0
  const absMax = Math.max(Math.abs(minPnl), Math.abs(maxPnl))
  const binSize = (absMax * 2) / bins
  const binStart = -absMax

  const histogram: { range: string; count: number; start: number; end: number }[] = []
  for (let i = 0; i < bins; i++) {
    const start = binStart + i * binSize
    const end = start + binSize
    const count = pnlValues.filter((v: number) => v >= start && v < end).length
    histogram.push({
      range: `$${start.toFixed(0)}`,
      count,
      start,
      end,
    })
  }

  // Stats
  const avgPnl = pnlValues.reduce((a: number, b: number) => a + b, 0) / pnlValues.length
  const winCount = pnlValues.filter((v: number) => v > 0).length
  const lossCount = pnlValues.filter((v: number) => v < 0).length

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <div className="flex justify-between items-center mb-3">
        <h3 className="text-lg font-semibold">P&L Distribution</h3>
        <div className="flex gap-4 text-xs text-gray-400">
          <span>
            <span className="text-green-400">{winCount}</span> wins
          </span>
          <span>
            <span className="text-red-400">{lossCount}</span> losses
          </span>
          <span>
            Avg: <span className={avgPnl >= 0 ? 'text-green-400' : 'text-red-400'}>
              ${avgPnl.toFixed(2)}
            </span>
          </span>
        </div>
      </div>

      <ResponsiveContainer width="100%" height={height}>
        <BarChart data={histogram} margin={{ top: 10, right: 10, bottom: 20, left: 10 }}>
          <XAxis
            dataKey="range"
            tick={{ fill: '#9CA3AF', fontSize: 10 }}
            axisLine={{ stroke: '#374151' }}
            tickLine={{ stroke: '#374151' }}
            interval={0}
            angle={-45}
            textAnchor="end"
            height={50}
          />
          <YAxis
            tick={{ fill: '#9CA3AF', fontSize: 11 }}
            axisLine={{ stroke: '#374151' }}
            tickLine={{ stroke: '#374151' }}
            allowDecimals={false}
          />
          <Tooltip
            contentStyle={{
              backgroundColor: '#1F2937',
              border: '1px solid #374151',
              borderRadius: '8px',
            }}
            labelStyle={{ color: '#E5E7EB' }}
            formatter={(value: any, name: string, props: any) => {
              const item = props.payload
              return [
                `${value} trades`,
                `$${item.start.toFixed(2)} to $${item.end.toFixed(2)}`,
              ]
            }}
          />
          <ReferenceLine x={5} stroke="#6B7280" strokeDasharray="3 3" />
          <Bar dataKey="count" radius={[4, 4, 0, 0]}>
            {histogram.map((entry, index) => (
              <Cell
                key={`cell-${index}`}
                fill={entry.start >= 0 ? '#10B981' : '#EF4444'}
                fillOpacity={0.8}
              />
            ))}
          </Bar>
        </BarChart>
      </ResponsiveContainer>
    </div>
  )
}
