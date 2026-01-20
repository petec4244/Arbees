import { useQuery } from '@tanstack/react-query'
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
  ReferenceLine,
  Area,
  ComposedChart,
} from 'recharts'
import { AlertTriangle, Zap } from 'lucide-react'

interface LatencyChartProps {
  height?: number
  showThresholds?: boolean
}

// Warning and critical thresholds in ms
const WARNING_THRESHOLD = 5000
const CRITICAL_THRESHOLD = 10000

export default function LatencyChart({
  height = 200,
  showThresholds = true,
}: LatencyChartProps) {
  const { data: latencyData, isLoading } = useQuery({
    queryKey: ['latencyMetrics'],
    queryFn: async () => {
      const res = await fetch('/api/monitoring/latency')
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

  if (isLoading) {
    return (
      <div className="bg-gray-800 rounded-lg p-4 h-64 flex items-center justify-center">
        <span className="text-gray-400">Loading latency data...</span>
      </div>
    )
  }

  // For now, show current metrics since we don't have time-series latency data
  // In a real implementation, you'd have a time-series endpoint
  const currentLatency = riskMetrics?.avg_latency_ms || 0
  const p95Latency = riskMetrics?.p95_latency_ms || 0
  const status = riskMetrics?.latency_status || 'good'

  // Mock time-series data for visualization (in production, this would come from API)
  const mockTimeSeriesData = Array.from({ length: 20 }, (_, i) => ({
    time: `${i * 3}m ago`,
    latency: currentLatency + (Math.random() - 0.5) * currentLatency * 0.5,
    p95: p95Latency + (Math.random() - 0.5) * p95Latency * 0.3,
  })).reverse()

  const getStatusColor = () => {
    switch (status) {
      case 'critical':
        return 'text-red-400'
      case 'warning':
        return 'text-yellow-400'
      default:
        return 'text-green-400'
    }
  }

  const getStatusBg = () => {
    switch (status) {
      case 'critical':
        return 'bg-red-900/30 border-red-700'
      case 'warning':
        return 'bg-yellow-900/30 border-yellow-700'
      default:
        return 'bg-green-900/30 border-green-700'
    }
  }

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <div className="flex justify-between items-center mb-4">
        <div className="flex items-center gap-2">
          <Zap className={`w-5 h-5 ${getStatusColor()}`} />
          <h3 className="text-lg font-semibold">Latency Monitor</h3>
        </div>
        <div className={`flex items-center gap-2 px-3 py-1 rounded border ${getStatusBg()}`}>
          {status === 'critical' && <AlertTriangle className="w-4 h-4 text-red-400 animate-pulse" />}
          <span className={`text-sm font-medium ${getStatusColor()}`}>
            {status.toUpperCase()}
          </span>
        </div>
      </div>

      {/* Current Stats */}
      <div className="grid grid-cols-3 gap-4 mb-4">
        <div className="bg-gray-900/50 rounded p-3">
          <div className="text-xs text-gray-500 uppercase">Avg Latency</div>
          <div className={`text-xl font-mono ${getStatusColor()}`}>
            {currentLatency.toFixed(0)}ms
          </div>
        </div>
        <div className="bg-gray-900/50 rounded p-3">
          <div className="text-xs text-gray-500 uppercase">P95 Latency</div>
          <div className="text-xl font-mono text-gray-300">
            {p95Latency.toFixed(0)}ms
          </div>
        </div>
        <div className="bg-gray-900/50 rounded p-3">
          <div className="text-xs text-gray-500 uppercase">Target</div>
          <div className="text-xl font-mono text-gray-400">
            &lt;{WARNING_THRESHOLD}ms
          </div>
        </div>
      </div>

      {/* Chart */}
      <ResponsiveContainer width="100%" height={height}>
        <ComposedChart data={mockTimeSeriesData} margin={{ top: 10, right: 10, bottom: 0, left: 0 }}>
          <XAxis
            dataKey="time"
            tick={{ fill: '#9CA3AF', fontSize: 10 }}
            axisLine={{ stroke: '#374151' }}
            tickLine={{ stroke: '#374151' }}
          />
          <YAxis
            tick={{ fill: '#9CA3AF', fontSize: 11 }}
            axisLine={{ stroke: '#374151' }}
            tickLine={{ stroke: '#374151' }}
            domain={[0, Math.max(CRITICAL_THRESHOLD, p95Latency * 1.2)]}
            tickFormatter={(value) => `${value}ms`}
          />
          <Tooltip
            contentStyle={{
              backgroundColor: '#1F2937',
              border: '1px solid #374151',
              borderRadius: '8px',
            }}
            labelStyle={{ color: '#E5E7EB' }}
            formatter={(value: any, name: string) => {
              return [`${value.toFixed(0)}ms`, name === 'latency' ? 'Avg' : 'P95']
            }}
          />
          {showThresholds && (
            <>
              <ReferenceLine
                y={WARNING_THRESHOLD}
                stroke="#EAB308"
                strokeDasharray="5 5"
                label={{
                  value: 'Warning',
                  position: 'right',
                  fill: '#EAB308',
                  fontSize: 10,
                }}
              />
              <ReferenceLine
                y={CRITICAL_THRESHOLD}
                stroke="#EF4444"
                strokeDasharray="5 5"
                label={{
                  value: 'Critical',
                  position: 'right',
                  fill: '#EF4444',
                  fontSize: 10,
                }}
              />
            </>
          )}
          <Area
            type="monotone"
            dataKey="p95"
            stroke="transparent"
            fill="rgba(156, 163, 175, 0.2)"
          />
          <Line
            type="monotone"
            dataKey="latency"
            stroke="#10B981"
            strokeWidth={2}
            dot={false}
          />
        </ComposedChart>
      </ResponsiveContainer>
    </div>
  )
}
