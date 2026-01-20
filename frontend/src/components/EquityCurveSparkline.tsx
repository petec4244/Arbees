import { useQuery } from '@tanstack/react-query'
import { Area, AreaChart, ResponsiveContainer, ReferenceLine } from 'recharts'

interface EquityCurveSparklineProps {
  days?: number
  height?: number
  showPeak?: boolean
}

export default function EquityCurveSparkline({
  days = 7,
  height = 60,
  showPeak = true,
}: EquityCurveSparklineProps) {
  const { data: equityHistory } = useQuery({
    queryKey: ['equityHistory', days],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/equity-history?days=${days}`)
      return res.json()
    },
    refetchInterval: 30000,
  })

  if (!equityHistory || equityHistory.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500 text-sm">
        No equity data
      </div>
    )
  }

  const latestEquity = equityHistory[equityHistory.length - 1]?.equity || 1000
  const peakEquity = Math.max(...equityHistory.map((d: any) => d.peak || d.equity))
  const minEquity = Math.min(...equityHistory.map((d: any) => d.equity))
  const startEquity = equityHistory[0]?.equity || 1000

  const isPositive = latestEquity >= startEquity
  const strokeColor = isPositive ? '#10B981' : '#EF4444'
  const fillColor = isPositive ? 'rgba(16, 185, 129, 0.2)' : 'rgba(239, 68, 68, 0.2)'

  return (
    <div className="relative">
      <ResponsiveContainer width="100%" height={height}>
        <AreaChart data={equityHistory} margin={{ top: 5, right: 5, bottom: 5, left: 5 }}>
          {showPeak && (
            <ReferenceLine y={peakEquity} stroke="#6B7280" strokeDasharray="3 3" />
          )}
          <Area
            type="monotone"
            dataKey="equity"
            stroke={strokeColor}
            fill={fillColor}
            strokeWidth={2}
            dot={false}
            isAnimationActive={false}
          />
        </AreaChart>
      </ResponsiveContainer>
      <div className="absolute right-0 top-0 flex items-center space-x-2">
        <span className={`text-sm font-mono ${isPositive ? 'text-green-400' : 'text-red-400'}`}>
          ${latestEquity.toFixed(2)}
        </span>
        {showPeak && latestEquity < peakEquity && (
          <span className="text-xs text-gray-500">(Peak: ${peakEquity.toFixed(2)})</span>
        )}
      </div>
    </div>
  )
}
