import { useQuery } from '@tanstack/react-query'
import {
  Area,
  AreaChart,
  CartesianGrid,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
  ReferenceLine,
} from 'recharts'

interface EquityCurveProps {
  days?: number
  height?: number
  showDrawdown?: boolean
}

export default function EquityCurve({
  days = 30,
  height = 300,
  showDrawdown = true,
}: EquityCurveProps) {
  const { data: equityHistory, isLoading } = useQuery({
    queryKey: ['equityHistory', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/equity-history?days=${days}`)
      return res.json()
    },
    refetchInterval: 30000,
  })

  if (isLoading) {
    return (
      <div className="flex items-center justify-center h-64 bg-gray-800 rounded-lg">
        <span className="text-gray-400">Loading equity curve...</span>
      </div>
    )
  }

  if (!equityHistory || equityHistory.length === 0) {
    return (
      <div className="flex items-center justify-center h-64 bg-gray-800 rounded-lg">
        <span className="text-gray-500">No equity history available</span>
      </div>
    )
  }

  const latestEquity = equityHistory[equityHistory.length - 1]?.equity || 1000
  const peakEquity = Math.max(...equityHistory.map((d: any) => d.peak || d.equity))
  const minEquity = Math.min(...equityHistory.map((d: any) => d.equity))
  const startEquity = equityHistory[0]?.equity || 1000
  const maxDrawdown = Math.max(...equityHistory.map((d: any) => d.drawdown_pct || 0))

  const isPositive = latestEquity >= startEquity
  const pnlChange = latestEquity - startEquity
  const pnlPct = ((latestEquity - startEquity) / startEquity * 100)

  // Format data for display
  const chartData = equityHistory.map((d: any) => ({
    ...d,
    date: new Date(d.time).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }),
    drawdownArea: d.peak - d.equity, // For drawdown shading
  }))

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      {/* Header Stats */}
      <div className="flex justify-between items-center mb-4">
        <div>
          <h3 className="text-lg font-semibold text-white">Equity Curve</h3>
          <p className="text-sm text-gray-400">{days} day performance</p>
        </div>
        <div className="text-right">
          <div className={`text-2xl font-bold font-mono ${isPositive ? 'text-green-400' : 'text-red-400'}`}>
            {isPositive ? '+' : ''}${pnlChange.toFixed(2)}
          </div>
          <div className={`text-sm ${isPositive ? 'text-green-400' : 'text-red-400'}`}>
            {isPositive ? '+' : ''}{pnlPct.toFixed(2)}%
          </div>
        </div>
      </div>

      {/* Summary Stats Row */}
      <div className="grid grid-cols-4 gap-4 mb-4 text-center">
        <div className="bg-gray-900/50 rounded p-2">
          <div className="text-xs text-gray-500 uppercase">Current</div>
          <div className="text-sm font-mono text-white">${latestEquity.toFixed(2)}</div>
        </div>
        <div className="bg-gray-900/50 rounded p-2">
          <div className="text-xs text-gray-500 uppercase">Peak</div>
          <div className="text-sm font-mono text-green-400">${peakEquity.toFixed(2)}</div>
        </div>
        <div className="bg-gray-900/50 rounded p-2">
          <div className="text-xs text-gray-500 uppercase">Low</div>
          <div className="text-sm font-mono text-yellow-400">${minEquity.toFixed(2)}</div>
        </div>
        <div className="bg-gray-900/50 rounded p-2">
          <div className="text-xs text-gray-500 uppercase">Max DD</div>
          <div className="text-sm font-mono text-red-400">-{maxDrawdown.toFixed(1)}%</div>
        </div>
      </div>

      {/* Chart */}
      <ResponsiveContainer width="100%" height={height}>
        <AreaChart data={chartData} margin={{ top: 10, right: 10, bottom: 0, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" stroke="#374151" />
          <XAxis
            dataKey="date"
            tick={{ fill: '#9CA3AF', fontSize: 11 }}
            axisLine={{ stroke: '#374151' }}
            tickLine={{ stroke: '#374151' }}
          />
          <YAxis
            tick={{ fill: '#9CA3AF', fontSize: 11 }}
            axisLine={{ stroke: '#374151' }}
            tickLine={{ stroke: '#374151' }}
            domain={['auto', 'auto']}
            tickFormatter={(value) => `$${value}`}
          />
          <Tooltip
            contentStyle={{
              backgroundColor: '#1F2937',
              border: '1px solid #374151',
              borderRadius: '8px',
            }}
            labelStyle={{ color: '#E5E7EB' }}
            formatter={(value: any, name: string) => {
              if (name === 'equity') return [`$${value.toFixed(2)}`, 'Equity']
              if (name === 'peak') return [`$${value.toFixed(2)}`, 'Peak']
              return [value, name]
            }}
          />
          {/* Peak reference line */}
          <ReferenceLine
            y={peakEquity}
            stroke="#10B981"
            strokeDasharray="5 5"
            label={{
              value: 'ATH',
              position: 'right',
              fill: '#10B981',
              fontSize: 10,
            }}
          />
          {/* Starting point reference */}
          <ReferenceLine y={startEquity} stroke="#6B7280" strokeDasharray="3 3" />
          {/* Drawdown shading */}
          {showDrawdown && (
            <Area
              type="monotone"
              dataKey="peak"
              stroke="transparent"
              fill="transparent"
              isAnimationActive={false}
            />
          )}
          {/* Main equity line */}
          <Area
            type="monotone"
            dataKey="equity"
            stroke={isPositive ? '#10B981' : '#EF4444'}
            fill={isPositive ? 'rgba(16, 185, 129, 0.3)' : 'rgba(239, 68, 68, 0.3)'}
            strokeWidth={2}
            dot={false}
            isAnimationActive={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  )
}
