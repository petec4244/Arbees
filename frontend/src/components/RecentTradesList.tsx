import { useQuery } from '@tanstack/react-query'
import { ArrowUpRight, ArrowDownRight, Clock } from 'lucide-react'

interface RecentTradesListProps {
  limit?: number
  compact?: boolean
}

export default function RecentTradesList({
  limit = 10,
  compact = false,
}: RecentTradesListProps) {
  const { data: trades, isLoading } = useQuery({
    queryKey: ['trades', 'recent', limit],
    queryFn: async () => {
      const res = await fetch(`/api/paper-trading/trades?limit=${limit}`)
      return res.json()
    },
    refetchInterval: 5000,
  })

  if (isLoading) {
    return (
      <div className="space-y-2">
        {Array.from({ length: 5 }).map((_, i) => (
          <div key={i} className="h-12 bg-gray-700 rounded animate-pulse" />
        ))}
      </div>
    )
  }

  if (!trades || trades.length === 0) {
    return (
      <div className="text-center py-8 text-gray-500">
        <Clock className="w-8 h-8 mx-auto mb-2 opacity-50" />
        <p>No recent trades</p>
      </div>
    )
  }

  if (compact) {
    return (
      <div className="space-y-2">
        {trades.map((trade: any) => (
          <div
            key={trade.trade_id}
            className="flex items-center justify-between p-2 bg-gray-700/50 rounded hover:bg-gray-700 transition-colors"
          >
            <div className="flex items-center gap-2">
              {trade.side === 'buy' ? (
                <ArrowUpRight className="w-4 h-4 text-green-400" />
              ) : (
                <ArrowDownRight className="w-4 h-4 text-red-400" />
              )}
              <span className="text-xs text-gray-400">
                {new Date(trade.entry_time).toLocaleTimeString()}
              </span>
            </div>
            <div className="text-right">
              {trade.pnl !== null ? (
                <span
                  className={`text-sm font-mono ${
                    trade.pnl >= 0 ? 'text-green-400' : 'text-red-400'
                  }`}
                >
                  {trade.pnl >= 0 ? '+' : ''}${trade.pnl.toFixed(2)}
                </span>
              ) : (
                <span className="text-xs px-1.5 py-0.5 bg-yellow-900/50 text-yellow-300 rounded">
                  OPEN
                </span>
              )}
            </div>
          </div>
        ))}
      </div>
    )
  }

  return (
    <div className="space-y-2">
      {trades.map((trade: any) => (
        <div
          key={trade.trade_id}
          className="flex items-center justify-between p-3 bg-gray-700/50 rounded-lg hover:bg-gray-700 transition-colors"
        >
          <div className="flex items-center gap-3">
            {trade.side === 'buy' ? (
              <div className="w-8 h-8 rounded-full bg-green-900/50 flex items-center justify-center" title="YES WIN">
                <ArrowUpRight className="w-4 h-4 text-green-400" />
              </div>
            ) : (
              <div className="w-8 h-8 rounded-full bg-red-900/50 flex items-center justify-center" title="NO LOSE">
                <ArrowDownRight className="w-4 h-4 text-red-400" />
              </div>
            )}
            <div>
              <div className="flex items-center gap-2">
                <span className="text-xs px-1.5 py-0.5 rounded bg-gray-600 text-gray-300 uppercase font-medium">
                  {trade.sport}
                </span>
                <span className="text-sm">
                  {trade.away_team && trade.home_team ? (
                    <>
                      <span className={`${
                        trade.entry_price < 0.5
                          ? 'text-orange-300 font-medium'
                          : 'text-orange-400/70'
                      }`}>
                        {trade.away_team}
                      </span>
                      <span className="text-gray-500"> @ </span>
                      <span className={`${
                        trade.entry_price >= 0.5
                          ? 'text-blue-300 font-medium'
                          : 'text-blue-400/70'
                      }`}>
                        {trade.home_team}
                      </span>
                    </>
                  ) : (
                    <span className="text-gray-300">{`Game ${trade.game_id?.slice(0, 8)}`}</span>
                  )}
                </span>
              </div>
              <div className="flex items-center gap-2 mt-0.5 text-xs text-gray-500">
                <span>{new Date(trade.entry_time).toLocaleTimeString()}</span>
                <span>|</span>
                <span className="font-mono">${trade.size.toFixed(2)}</span>
                {trade.edge_at_entry && (
                  <>
                    <span>|</span>
                    <span className="text-green-400">{trade.edge_at_entry.toFixed(1)}% edge</span>
                  </>
                )}
              </div>
            </div>
          </div>

          <div className="text-right">
            {trade.pnl !== null ? (
              <div>
                <span
                  className={`text-lg font-mono font-semibold ${
                    trade.pnl >= 0 ? 'text-green-400' : 'text-red-400'
                  }`}
                >
                  {trade.pnl >= 0 ? '+' : ''}${trade.pnl.toFixed(2)}
                </span>
                {trade.pnl_pct && (
                  <div className={`text-xs ${trade.pnl >= 0 ? 'text-green-400' : 'text-red-400'}`}>
                    {trade.pnl_pct >= 0 ? '+' : ''}{trade.pnl_pct.toFixed(1)}%
                  </div>
                )}
              </div>
            ) : (
              <span className="text-xs px-2 py-1 bg-yellow-900/50 text-yellow-300 rounded border border-yellow-700">
                OPEN
              </span>
            )}
          </div>
        </div>
      ))}
    </div>
  )
}
