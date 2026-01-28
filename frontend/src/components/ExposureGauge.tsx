import { useQuery } from '@tanstack/react-query'

interface ExposureGaugeProps {
  label: string
  value: number
  limit: number
  showLabel?: boolean
  size?: 'sm' | 'md' | 'lg'
}

export function ExposureGauge({
  label,
  value,
  limit,
  showLabel = true,
  size = 'md',
}: ExposureGaugeProps) {
  const pct = limit > 0 ? (value / limit * 100) : 0

  const getBarColor = () => {
    if (pct >= 90) return 'bg-red-500'
    if (pct >= 70) return 'bg-yellow-500'
    return 'bg-green-500'
  }

  const heightClass = size === 'sm' ? 'h-1.5' : size === 'lg' ? 'h-3' : 'h-2'

  return (
    <div className="space-y-1">
      {showLabel && (
        <div className="flex justify-between items-center text-xs">
          <span className="text-gray-400 uppercase tracking-wider">{label}</span>
          <span className="text-gray-300 font-mono">
            ${value.toFixed(0)} / ${limit.toFixed(0)}
            <span className="text-gray-500 ml-1">({pct.toFixed(0)}%)</span>
          </span>
        </div>
      )}
      <div className={`${heightClass} bg-gray-700 rounded-full overflow-hidden`}>
        <div
          className={`h-full ${getBarColor()} transition-all duration-300`}
          style={{ width: `${Math.min(pct, 100)}%` }}
        />
      </div>
    </div>
  )
}

interface ExposureBySportProps {
  showHeader?: boolean
}

export function ExposureBySport({ showHeader = true }: ExposureBySportProps) {
  const { data: riskMetrics } = useQuery({
    queryKey: ['riskMetrics'],
    queryFn: async () => {
      const res = await fetch('/api/risk/metrics')
      return res.json()
    },
    refetchInterval: 5000,
  })

  if (!riskMetrics?.exposure_by_sport || Object.keys(riskMetrics.exposure_by_sport).length === 0) {
    return (
      <div className="bg-gray-800 rounded-lg p-4">
        {showHeader && <h3 className="text-lg font-semibold mb-3">Exposure by Market</h3>}
        <p className="text-gray-500 text-sm">No open positions</p>
      </div>
    )
  }

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      {showHeader && <h3 className="text-lg font-semibold mb-3">Exposure by Market</h3>}
      <div className="space-y-3">
        {Object.entries(riskMetrics.exposure_by_sport).map(([sport, data]: [string, any]) => (
          <ExposureGauge
            key={sport}
            label={sport.toUpperCase()}
            value={data.exposure}
            limit={data.limit}
          />
        ))}
      </div>
    </div>
  )
}

interface ExposureByGameProps {
  showHeader?: boolean
  limit?: number
}

export function ExposureByGame({ showHeader = true, limit = 5 }: ExposureByGameProps) {
  const { data: riskMetrics } = useQuery({
    queryKey: ['riskMetrics'],
    queryFn: async () => {
      const res = await fetch('/api/risk/metrics')
      return res.json()
    },
    refetchInterval: 5000,
  })

  if (!riskMetrics?.exposure_by_game || Object.keys(riskMetrics.exposure_by_game).length === 0) {
    return (
      <div className="bg-gray-800 rounded-lg p-4">
        {showHeader && <h3 className="text-lg font-semibold mb-3">Exposure by Event</h3>}
        <p className="text-gray-500 text-sm">No open positions</p>
      </div>
    )
  }

  const games = Object.entries(riskMetrics.exposure_by_game).slice(0, limit)

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      {showHeader && <h3 className="text-lg font-semibold mb-3">Exposure by Event</h3>}
      <div className="overflow-x-auto">
        <table className="min-w-full">
          <thead>
            <tr className="text-xs text-gray-500 uppercase">
              <th className="text-left pb-2">Game</th>
              <th className="text-right pb-2">Exposure</th>
              <th className="text-right pb-2">Limit</th>
              <th className="text-left pb-2 pl-4 w-32">Status</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-700">
            {games.map(([gameId, data]: [string, any]) => (
              <tr key={gameId}>
                <td className="py-2 text-sm text-gray-300">{data.name}</td>
                <td className="py-2 text-sm text-right font-mono">${data.exposure.toFixed(2)}</td>
                <td className="py-2 text-sm text-right font-mono text-gray-500">${data.limit.toFixed(2)}</td>
                <td className="py-2 pl-4">
                  <div className="h-2 w-24 bg-gray-700 rounded-full overflow-hidden">
                    <div
                      className={`h-full ${data.pct >= 90 ? 'bg-red-500' : data.pct >= 70 ? 'bg-yellow-500' : 'bg-green-500'
                        }`}
                      style={{ width: `${Math.min(data.pct, 100)}%` }}
                    />
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  )
}
