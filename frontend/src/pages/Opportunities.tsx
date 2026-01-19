import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

export default function Opportunities() {
  const [minEdge, setMinEdge] = useState(1.0)
  const [sport, setSport] = useState('')

  const { data: opportunities, isLoading } = useQuery({
    queryKey: ['opportunities', minEdge, sport],
    queryFn: async () => {
      const params = new URLSearchParams()
      params.set('min_edge', minEdge.toString())
      if (sport) params.set('sport', sport)
      const res = await fetch(`/api/opportunities?${params}`)
      return res.json()
    },
  })

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Arbitrage Opportunities</h1>

      {/* Filters */}
      <div className="flex gap-4 items-center">
        <div>
          <label className="block text-sm text-gray-400 mb-1">Min Edge %</label>
          <input
            type="number"
            value={minEdge}
            onChange={(e) => setMinEdge(parseFloat(e.target.value) || 0)}
            className="bg-gray-700 rounded px-3 py-2 w-24"
            step="0.5"
          />
        </div>
        <div>
          <label className="block text-sm text-gray-400 mb-1">Sport</label>
          <select
            value={sport}
            onChange={(e) => setSport(e.target.value)}
            className="bg-gray-700 rounded px-3 py-2"
          >
            <option value="">All Sports</option>
            <option value="nfl">NFL</option>
            <option value="nba">NBA</option>
            <option value="nhl">NHL</option>
            <option value="mlb">MLB</option>
          </select>
        </div>
      </div>

      {/* Opportunities Table */}
      <div className="bg-gray-800 rounded-lg overflow-hidden">
        <table className="min-w-full divide-y divide-gray-700">
          <thead className="bg-gray-700">
            <tr>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Market</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Type</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Buy</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Sell</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Edge</th>
              <th className="px-6 py-3 text-left text-xs font-medium text-gray-300 uppercase">Liquidity</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-gray-700">
            {isLoading && (
              <tr>
                <td colSpan={6} className="px-6 py-4 text-center text-gray-400">Loading...</td>
              </tr>
            )}
            {opportunities?.map((opp: any) => (
              <tr key={opp.opportunity_id} className="hover:bg-gray-700">
                <td className="px-6 py-4 whitespace-nowrap">
                  <div className="text-sm">{opp.market_title}</div>
                  <div className="text-xs text-gray-400">{opp.sport}</div>
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm">
                  <span className={`px-2 py-1 rounded text-xs ${opp.is_risk_free ? 'bg-green-900 text-green-300' : 'bg-yellow-900 text-yellow-300'}`}>
                    {opp.opportunity_type}
                  </span>
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm">
                  <div>{opp.platform_buy}</div>
                  <div className="text-green-400">{(opp.buy_price * 100).toFixed(1)}¢</div>
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm">
                  <div>{opp.platform_sell}</div>
                  <div className="text-red-400">{(opp.sell_price * 100).toFixed(1)}¢</div>
                </td>
                <td className="px-6 py-4 whitespace-nowrap">
                  <span className="text-green-400 font-mono text-lg">{opp.edge_pct.toFixed(2)}%</span>
                </td>
                <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-400">
                  ${Math.min(opp.liquidity_buy, opp.liquidity_sell).toFixed(0)}
                </td>
              </tr>
            ))}
            {(!isLoading && (!opportunities || opportunities.length === 0)) && (
              <tr>
                <td colSpan={6} className="px-6 py-4 text-center text-gray-400">No opportunities found</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  )
}
