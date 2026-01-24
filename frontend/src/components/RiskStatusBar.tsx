import { useQuery } from '@tanstack/react-query'
import { AlertTriangle, CheckCircle, Clock, Zap } from 'lucide-react'

interface RiskStatusBarProps {
  compact?: boolean
}

export default function RiskStatusBar({ compact = true }: RiskStatusBarProps) {
  const { data: riskMetrics } = useQuery({
    queryKey: ['riskMetrics'],
    queryFn: async () => {
      const res = await fetch('/api/risk/metrics')
      return res.json()
    },
    refetchInterval: 5000,
  })

  if (!riskMetrics) {
    return (
      <div className="bg-gray-800 rounded-lg p-3 animate-pulse">
        <div className="h-4 bg-gray-700 rounded w-full"></div>
      </div>
    )
  }

  const {
    daily_pnl,
    daily_limit,
    daily_limit_pct,
    total_exposure,
    max_exposure,
    circuit_breaker_open,
    avg_latency_ms,
    latency_status,
  } = riskMetrics

  const exposurePct = max_exposure > 0 ? (total_exposure / max_exposure * 100) : 0

  const getLatencyColor = () => {
    if (latency_status === 'critical') return 'text-red-400'
    if (latency_status === 'warning') return 'text-yellow-400'
    return 'text-green-400'
  }

  const getProgressBarColor = (pct: number) => {
    if (pct >= 90) return 'bg-red-500'
    if (pct >= 70) return 'bg-yellow-500'
    return 'bg-green-500'
  }

  if (compact) {
    return (
      <div className="bg-gray-800/50 rounded-lg p-3 border border-gray-700">
        <div className="flex items-center justify-between gap-4 flex-wrap">
          {/* Piggybank */}
          <div className="flex items-center gap-2 mr-2">
            <span className="text-lg">🐷</span>
            <div className="flex flex-col leading-none">
              <span className="text-[10px] text-gray-400 uppercase font-bold">Piggybank</span>
              <span className="text-sm font-mono text-pink-400 font-bold">
                ${(riskMetrics.piggybank_balance || 0).toFixed(2)}
              </span>
            </div>
          </div>

          {/* Daily Limit */}
          <div className="flex items-center gap-3 min-w-[200px]">
            <span className="text-xs text-gray-400 whitespace-nowrap">Daily Limit:</span>
            <div className="flex-1 h-2 bg-gray-700 rounded-full overflow-hidden">
              <div
                className={`h-full ${getProgressBarColor(daily_limit_pct)} transition-all`}
                style={{ width: `${Math.min(daily_limit_pct, 100)}%` }}
              />
            </div>
            <span className="text-xs text-gray-300 font-mono">{daily_limit_pct.toFixed(0)}%</span>
          </div>

          {/* Exposure */}
          <div className="flex items-center gap-3 min-w-[200px]">
            <span className="text-xs text-gray-400 whitespace-nowrap">Exposure:</span>
            <div className="flex-1 h-2 bg-gray-700 rounded-full overflow-hidden">
              <div
                className={`h-full ${getProgressBarColor(exposurePct)} transition-all`}
                style={{ width: `${Math.min(exposurePct, 100)}%` }}
              />
            </div>
            <span className="text-xs text-gray-300 font-mono">
              ${total_exposure.toFixed(0)}/${max_exposure.toFixed(0)}
            </span>
          </div>

          {/* Circuit Breaker */}
          <div className="flex items-center gap-2">
            <span className="text-xs text-gray-400">Circuit:</span>
            {circuit_breaker_open ? (
              <div className="flex items-center gap-1 text-red-400">
                <AlertTriangle className="w-4 h-4" />
                <span className="text-xs font-medium">OPEN</span>
              </div>
            ) : (
              <div className="flex items-center gap-1 text-green-400">
                <CheckCircle className="w-4 h-4" />
                <span className="text-xs font-medium">CLOSED</span>
              </div>
            )}
          </div>

          {/* Latency */}
          <div className="flex items-center gap-2">
            <Zap className={`w-4 h-4 ${getLatencyColor()}`} />
            <span className={`text-xs font-mono ${getLatencyColor()}`}>
              {avg_latency_ms.toFixed(0)}ms
            </span>
          </div>
        </div>
      </div>
    )
  }

  // Detailed view
  return (
    <div className="bg-gray-800 rounded-lg p-4 space-y-4">
      <h3 className="text-lg font-semibold flex items-center gap-2">
        <AlertTriangle className="w-5 h-5 text-yellow-400" />
        Risk Status
      </h3>

      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        {/* Daily P&L */}
        <div className="bg-gray-900/50 rounded p-3">
          <div className="flex items-center gap-2 text-gray-400 text-xs mb-1">
            <Clock className="w-3 h-3" />
            Daily P&L
          </div>
          <div className={`text-lg font-mono ${daily_pnl >= 0 ? 'text-green-400' : 'text-red-400'}`}>
            {daily_pnl >= 0 ? '+' : ''}${daily_pnl.toFixed(2)}
          </div>
          <div className="mt-2">
            <div className="flex justify-between text-xs text-gray-500 mb-1">
              <span>Limit used</span>
              <span>{daily_limit_pct.toFixed(0)}%</span>
            </div>
            <div className="h-1.5 bg-gray-700 rounded-full overflow-hidden">
              <div
                className={`h-full ${getProgressBarColor(daily_limit_pct)} transition-all`}
                style={{ width: `${Math.min(daily_limit_pct, 100)}%` }}
              />
            </div>
          </div>
        </div>

        {/* Piggybank */}
        <div className="bg-gray-900/50 rounded p-3 relative overflow-hidden group">
          <div className="absolute top-0 right-0 p-3 opacity-10 group-hover:opacity-20 transition-opacity">
            <span className="text-4xl">🐷</span>
          </div>
          <div className="flex items-center gap-2 text-gray-400 text-xs mb-1">
            <span>Piggybank</span>
          </div>
          <div className="text-lg font-mono text-pink-400">
            ${(riskMetrics.piggybank_balance || 0).toFixed(2)}
          </div>
          <div className="text-xs text-gray-500 mt-2">
            Safety Buffer (50%)
          </div>
        </div>

        {/* Exposure */}
        <div className="bg-gray-900/50 rounded p-3">
          <div className="text-gray-400 text-xs mb-1">Total Exposure</div>
          <div className="text-lg font-mono text-white">${total_exposure.toFixed(2)}</div>
          <div className="mt-2">
            <div className="flex justify-between text-xs text-gray-500 mb-1">
              <span>Of max</span>
              <span>${max_exposure.toFixed(0)}</span>
            </div>
            <div className="h-1.5 bg-gray-700 rounded-full overflow-hidden">
              <div
                className={`h-full ${getProgressBarColor(exposurePct)} transition-all`}
                style={{ width: `${Math.min(exposurePct, 100)}%` }}
              />
            </div>
          </div>
        </div>

        {/* Circuit Breaker */}
        <div className="bg-gray-900/50 rounded p-3">
          <div className="text-gray-400 text-xs mb-1">Circuit Breaker</div>
          {circuit_breaker_open ? (
            <div className="flex items-center gap-2">
              <AlertTriangle className="w-5 h-5 text-red-400 animate-pulse" />
              <span className="text-lg font-medium text-red-400">OPEN</span>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              <CheckCircle className="w-5 h-5 text-green-400" />
              <span className="text-lg font-medium text-green-400">CLOSED</span>
            </div>
          )}
          <div className="text-xs text-gray-500 mt-2">
            {circuit_breaker_open ? 'Trading halted' : 'Normal operations'}
          </div>
        </div>

        {/* Latency */}
        <div className="bg-gray-900/50 rounded p-3">
          <div className="text-gray-400 text-xs mb-1">Avg Latency</div>
          <div className={`text-lg font-mono ${getLatencyColor()}`}>
            {avg_latency_ms.toFixed(0)}ms
          </div>
          <div className="text-xs text-gray-500 mt-2">
            Status: <span className={getLatencyColor()}>{latency_status}</span>
          </div>
        </div>
      </div>
    </div>
  )
}
