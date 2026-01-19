import { useQuery } from '@tanstack/react-query'

async function fetchOpportunityStats() {
  const res = await fetch('/api/opportunities/stats')
  return res.json()
}

async function fetchPerformance() {
  const res = await fetch('/api/paper-trading/performance')
  return res.json()
}

async function fetchLiveGames() {
  const res = await fetch('/api/live-games')
  return res.json()
}

export default function Dashboard() {
  const { data: stats } = useQuery({
    queryKey: ['opportunityStats'],
    queryFn: fetchOpportunityStats,
  })

  const { data: performance } = useQuery({
    queryKey: ['performance'],
    queryFn: fetchPerformance,
  })

  const { data: games } = useQuery({
    queryKey: ['liveGames'],
    queryFn: fetchLiveGames,
  })

  return (
    <div className="space-y-6">
      <h1 className="text-3xl font-bold">Dashboard</h1>

      {/* Stats Cards */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <StatCard
          title="Active Opportunities"
          value={stats?.total_active || 0}
          subtext={`Avg edge: ${(stats?.avg_edge || 0).toFixed(2)}%`}
        />
        <StatCard
          title="Live Games"
          value={games?.length || 0}
          subtext="Across all sports"
        />
        <StatCard
          title="Win Rate"
          value={`${(performance?.win_rate || 0).toFixed(1)}%`}
          subtext={`${performance?.total_trades || 0} trades`}
        />
        <StatCard
          title="Total P&L"
          value={`$${(performance?.total_pnl || 0).toFixed(2)}`}
          subtext={`ROI: ${(performance?.roi_pct || 0).toFixed(1)}%`}
          positive={(performance?.total_pnl || 0) > 0}
        />
      </div>

      {/* Recent Activity */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        <div className="bg-gray-800 rounded-lg p-6">
          <h2 className="text-xl font-semibold mb-4">Live Games</h2>
          <div className="space-y-3">
            {games?.slice(0, 5).map((game: any) => (
              <div key={game.game_id} className="flex justify-between items-center p-3 bg-gray-700 rounded">
                <span>{game.away_team_abbrev} @ {game.home_team_abbrev}</span>
                <span className="font-mono">{game.away_score} - {game.home_score}</span>
              </div>
            ))}
            {(!games || games.length === 0) && (
              <p className="text-gray-400">No live games at the moment</p>
            )}
          </div>
        </div>

        <div className="bg-gray-800 rounded-lg p-6">
          <h2 className="text-xl font-semibold mb-4">Top Opportunities</h2>
          <OpportunityList limit={5} />
        </div>
      </div>
    </div>
  )
}

function StatCard({
  title,
  value,
  subtext,
  positive,
}: {
  title: string
  value: string | number
  subtext: string
  positive?: boolean
}) {
  return (
    <div className="bg-gray-800 rounded-lg p-6">
      <p className="text-gray-400 text-sm">{title}</p>
      <p className={`text-3xl font-bold mt-2 ${positive !== undefined ? (positive ? 'text-green-400' : 'text-red-400') : ''}`}>
        {value}
      </p>
      <p className="text-gray-500 text-sm mt-1">{subtext}</p>
    </div>
  )
}

function OpportunityList({ limit }: { limit: number }) {
  const { data: opportunities } = useQuery({
    queryKey: ['opportunities', limit],
    queryFn: async () => {
      const res = await fetch(`/api/opportunities?limit=${limit}`)
      return res.json()
    },
  })

  return (
    <div className="space-y-3">
      {opportunities?.map((opp: any) => (
        <div key={opp.opportunity_id} className="flex justify-between items-center p-3 bg-gray-700 rounded">
          <div>
            <span className="text-sm">{opp.market_title}</span>
            <span className="text-xs text-gray-400 block">
              {opp.platform_buy} → {opp.platform_sell}
            </span>
          </div>
          <span className="text-green-400 font-mono">{opp.edge_pct.toFixed(2)}%</span>
        </div>
      ))}
      {(!opportunities || opportunities.length === 0) && (
        <p className="text-gray-400">No active opportunities</p>
      )}
    </div>
  )
}
