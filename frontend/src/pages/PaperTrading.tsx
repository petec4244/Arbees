import { useQuery } from '@tanstack/react-query'

export default function PaperTrading() {
  const { data: performance } = useQuery({
    queryKey: ['performance'],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/performance')
      return res.json()
    },
  })

  const { data: trades } = useQuery({
    queryKey: ['trades'],
    queryFn: async () => {
      const res = await fetch('/api/paper-trading/trades?limit=50')
      return res.json()
    },
  })

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Paper Trading</h1>

      {/* Performance Summary */}
      <div className="grid grid-cols-1 md:grid-cols-4 gap-4">
        <StatCard
          title="Current Bankroll"
          value={`$${(performance?.current_bankroll || 1000).toFixed(2)}`}
        />
        <StatCard
          title="Total P&L"
          value={`$${(performance?.total_pnl || 0).toFixed(2)}`}
          className={(performance?.total_pnl || 0) >= 0 ? 'text-green-400' : 'text-red-400'}
        />
        <StatCard
          title="Win Rate"
          value={`${(performance?.win_rate || 0).toFixed(1)}%`}
        />
        <StatCard
          title="Total Trades"
          value={performance?.total_trades || 0}
        />
      </div>

      {/* Trade History */}
      <div className="bg-gray-800 rounded-lg overflow-hidden">
        <h2 className="text-xl font-semibold p-4 border-b border-gray-700">Trade History</h2>
        <table className="min-w-full divide-y divide-gray-700">
          <thead className="bg-gray-700">
            <tr>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Time</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Market</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Side</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Entry</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Exit</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">P&L</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Status</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-700">
            {trades?.map((trade: any) => (
              <tr key={trade.trade_id} className="hover:bg-gray-700">
                <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-400">
                  {new Date(trade.entry_time).toLocaleString()}
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm">
                  <div>{trade.market_title || trade.market_id}</div>
                  <div className="text-xs text-gray-400">{trade.platform}</div>
                </td>
                <td className="px-6 py-4 whitespace-nowrap">
                  <span className={`px-2 py-1 rounded text-xs ${trade.side === 'buy' ? 'bg-green-900 text-green-300' : 'bg-red-900 text-red-300'}`}>
                    {trade.side.toUpperCase()}
                  </span>
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm">
                  {(trade.entry_price * 100).toFixed(1)}¢
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm">
                  {trade.exit_price ? `${(trade.exit_price * 100).toFixed(1)}¢` : '-'}
                </td>
                <td className="px-6 py-4 whitespace-nowrap">
                  {trade.pnl !== null ? (
                    <span className={trade.pnl >= 0 ? 'text-green-400' : 'text-red-400'}>
                      ${trade.pnl.toFixed(2)} ({trade.pnl_pct?.toFixed(1)}%)
                    </span>
                  ) : '-'}
                </td>
                <td className="px-6 py-4 whitespace-nowrap">
                  <span className={`px-2 py-1 rounded text-xs ${
                    trade.status === 'closed'
                      ? (trade.outcome === 'win' ? 'bg-green-900 text-green-300' : 'bg-red-900 text-red-300')
                      : 'bg-yellow-900 text-yellow-300'
                  }`}>
                    {trade.status === 'closed' ? trade.outcome : trade.status}
                  </span>
                </td>
              </tr>
            ))}
            {(!trades || trades.length === 0) && (
              <tr>
                <td colSpan={7} className="px-6 py-4 text-center text-gray-400">No trades yet</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  )
}

function StatCard({ title, value, className }: { title: string; value: string | number; className?: string }) {
  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <p className="text-gray-400 text-sm">{title}</p>
      <p className={`text-2xl font-bold mt-1 ${className || ''}`}>{value}</p>
    </div>
  )
}
