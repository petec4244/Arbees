import { useQuery } from '@tanstack/react-query'
import { TrendingUp, TrendingDown, Activity, Target, AlertTriangle, ChevronDown, ChevronUp, Eye, EyeOff } from 'lucide-react'
import EquityCurveSparkline from '../components/EquityCurveSparkline'
import RiskStatusBar from '../components/RiskStatusBar'
import RecentTradesList from '../components/RecentTradesList'
import { useUIPreferences } from '../hooks/useUIPreferences'

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

async function fetchRiskMetrics() {
  const res = await fetch('/api/risk/metrics')
  return res.json()
}

export default function Dashboard() {
  const {
    showLiveGames,
    showRecentTrades,
    showOpportunities,
    showEquityCurve,
    showRiskBar,
    toggleSection,
    isSectionCollapsed,
  } = useUIPreferences()

  const { data: stats } = useQuery({
    queryKey: ['opportunityStats'],
    queryFn: fetchOpportunityStats,
    refetchInterval: 10000,
  })

  const { data: performance } = useQuery({
    queryKey: ['performance'],
    queryFn: fetchPerformance,
    refetchInterval: 10000,
  })

  const { data: games } = useQuery({
    queryKey: ['liveGames'],
    queryFn: fetchLiveGames,
    refetchInterval: 5000,
  })

  const { data: riskMetrics } = useQuery({
    queryKey: ['riskMetrics'],
    queryFn: fetchRiskMetrics,
    refetchInterval: 5000,
  })

  // Calculate daily P&L (from risk metrics)
  const dailyPnl = riskMetrics?.daily_pnl || 0
  const maxDrawdown = riskMetrics?.daily_limit_pct || 0

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <h1 className="text-3xl font-bold">Dashboard</h1>
        <ViewToggle />
      </div>

      {/* Header KPI Cards - 5 cards */}
      <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-4">
        <StatCard
          title="Total P&L"
          value={`$${(performance?.total_pnl || 0).toFixed(2)}`}
          subtext={`ROI: ${(performance?.roi_pct || 0).toFixed(1)}%`}
          positive={(performance?.total_pnl || 0) > 0}
          icon={<TrendingUp className="w-4 h-4" />}
        />
        <StatCard
          title="Daily P&L"
          value={`$${dailyPnl.toFixed(2)}`}
          subtext="Today"
          positive={dailyPnl >= 0}
          icon={dailyPnl >= 0 ? <TrendingUp className="w-4 h-4" /> : <TrendingDown className="w-4 h-4" />}
        />
        <StatCard
          title="Win Rate"
          value={`${(performance?.win_rate || 0).toFixed(1)}%`}
          subtext={`${performance?.winning_trades || 0}/${performance?.total_trades || 0}`}
          icon={<Target className="w-4 h-4 text-blue-400" />}
        />
        <StatCard
          title="Active Games"
          value={games?.length || 0}
          subtext={`${stats?.total_active || 0} opportunities`}
          icon={<Activity className="w-4 h-4 text-green-400" />}
        />
        <StatCard
          title="Drawdown"
          value={`-${maxDrawdown.toFixed(1)}%`}
          subtext="Daily limit used"
          negative={maxDrawdown > 50}
          icon={<AlertTriangle className="w-4 h-4 text-yellow-400" />}
        />
      </div>

      {/* Mini Equity Curve */}
      {showEquityCurve && (
        <div className="bg-gray-800 rounded-lg p-4">
          <div className="flex justify-between items-center mb-2">
            <h2 className="text-sm font-medium text-gray-400">7-Day Equity Trend</h2>
          </div>
          <EquityCurveSparkline days={7} height={80} showPeak />
        </div>
      )}

      {/* Risk Status Bar */}
      {showRiskBar && <RiskStatusBar compact />}

      {/* Two-Column Layout */}
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Live Games */}
        {showLiveGames && (
          <CollapsibleSection
            title="Live Games"
            badge={`${games?.length || 0} active`}
            sectionId="dashboard-live-games"
          >
            <div className="space-y-3 max-h-[400px] overflow-y-auto pr-2 custom-scrollbar">
              {games?.map((game: any) => (
                <div
                  key={game.game_id}
                  className="flex flex-col p-3 bg-gray-700/50 rounded hover:bg-gray-700 transition-colors"
                >
                  <div className="flex justify-between items-center mb-2">
                    <span className="text-xs font-bold text-gray-400 uppercase tracking-wider">
                      {game.sport}
                    </span>
                    <span className="text-xs text-green-400 font-mono animate-pulse">
                      <span className="inline-block w-2 h-2 bg-green-400 rounded-full mr-1" />
                      LIVE
                    </span>
                  </div>
                  <div className="flex justify-between items-center">
                    <div className="flex-1">
                      <div className="flex justify-between">
                        <span className="text-gray-300">{game.away_team}</span>
                        <span className="font-mono font-bold">{game.away_score}</span>
                      </div>
                      <div className="flex justify-between mt-1">
                        <span className="text-white font-medium">{game.home_team}</span>
                        <span className="font-mono font-bold">{game.home_score}</span>
                      </div>
                    </div>
                  </div>
                  {game.home_win_prob !== null && (
                    <div className="mt-2 flex items-center gap-2">
                      <div className="flex-1 h-1.5 bg-gray-600 rounded-full overflow-hidden">
                        <div
                          className="h-full bg-blue-500 transition-all"
                          style={{ width: `${(game.home_win_prob || 0.5) * 100}%` }}
                        />
                      </div>
                      <span className="text-xs text-gray-400 font-mono">
                        {((game.home_win_prob || 0.5) * 100).toFixed(0)}%
                      </span>
                    </div>
                  )}
                  {game.status && (
                    <div className="mt-2 text-xs text-gray-500 text-right">
                      {game.status.replace('_', ' ')}
                    </div>
                  )}
                </div>
              ))}
              {(!games || games.length === 0) && (
                <div className="flex flex-col items-center justify-center h-48 text-gray-500">
                  <Activity className="w-8 h-8 mb-2 opacity-50" />
                  <p>No live games at the moment</p>
                </div>
              )}
            </div>
          </CollapsibleSection>
        )}

        {/* Recent Trades */}
        {showRecentTrades && (
          <CollapsibleSection
            title="Recent Trades"
            badge="Last 10"
            sectionId="dashboard-recent-trades"
          >
            <RecentTradesList limit={10} compact={false} />
          </CollapsibleSection>
        )}
      </div>

      {/* Top Opportunities */}
      {showOpportunities && (
        <CollapsibleSection
          title="Top Opportunities"
          sectionId="dashboard-opportunities"
        >
          <OpportunityList limit={5} />
        </CollapsibleSection>
      )}
    </div>
  )
}

function StatCard({
  title,
  value,
  subtext,
  positive,
  negative,
  icon,
}: {
  title: string
  value: string | number
  subtext: string
  positive?: boolean
  negative?: boolean
  icon?: React.ReactNode
}) {
  const getValueColor = () => {
    if (positive !== undefined) return positive ? 'text-green-400' : 'text-red-400'
    if (negative !== undefined) return negative ? 'text-red-400' : 'text-yellow-400'
    return ''
  }

  return (
    <div className="bg-gray-800 rounded-lg p-4">
      <div className="flex items-center gap-2 text-gray-400 text-sm mb-1">
        {icon}
        <span>{title}</span>
      </div>
      <p className={`text-2xl font-bold font-mono ${getValueColor()}`}>{value}</p>
      <p className="text-gray-500 text-xs mt-1">{subtext}</p>
    </div>
  )
}

function CollapsibleSection({
  title,
  badge,
  sectionId,
  children,
}: {
  title: string
  badge?: string
  sectionId: string
  children: React.ReactNode
}) {
  const { isSectionCollapsed, toggleSection } = useUIPreferences()
  const collapsed = isSectionCollapsed(sectionId)

  return (
    <div className="bg-gray-800 rounded-lg overflow-hidden">
      <button
        onClick={() => toggleSection(sectionId)}
        className="w-full p-4 flex justify-between items-center hover:bg-gray-700/50 transition-colors"
      >
        <div className="flex items-center gap-3">
          <h2 className="text-xl font-semibold">{title}</h2>
          {badge && (
            <span className="text-xs text-gray-400 bg-gray-700 px-2 py-1 rounded-full">
              {badge}
            </span>
          )}
        </div>
        {collapsed ? (
          <ChevronDown className="w-5 h-5 text-gray-400" />
        ) : (
          <ChevronUp className="w-5 h-5 text-gray-400" />
        )}
      </button>
      {!collapsed && <div className="p-4 pt-0">{children}</div>}
    </div>
  )
}

function ViewToggle() {
  const {
    showLiveGames,
    setShowLiveGames,
    showRecentTrades,
    setShowRecentTrades,
    showOpportunities,
    setShowOpportunities,
    showEquityCurve,
    setShowEquityCurve,
    showRiskBar,
    setShowRiskBar,
  } = useUIPreferences()

  const toggles = [
    { label: 'Equity', value: showEquityCurve, set: setShowEquityCurve },
    { label: 'Risk', value: showRiskBar, set: setShowRiskBar },
    { label: 'Games', value: showLiveGames, set: setShowLiveGames },
    { label: 'Trades', value: showRecentTrades, set: setShowRecentTrades },
    { label: 'Opps', value: showOpportunities, set: setShowOpportunities },
  ]

  return (
    <div className="flex items-center gap-1 bg-gray-800 rounded-lg p-1">
      {toggles.map((toggle) => (
        <button
          key={toggle.label}
          onClick={() => toggle.set(!toggle.value)}
          className={`p-2 rounded text-xs transition-colors ${
            toggle.value
              ? 'bg-gray-700 text-white'
              : 'text-gray-500 hover:text-gray-300'
          }`}
          title={`${toggle.value ? 'Hide' : 'Show'} ${toggle.label}`}
        >
          {toggle.value ? (
            <Eye className="w-4 h-4" />
          ) : (
            <EyeOff className="w-4 h-4" />
          )}
        </button>
      ))}
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
    refetchInterval: 10000,
  })

  return (
    <div className="space-y-3">
      {opportunities?.map((opp: any) => (
        <div
          key={opp.opportunity_id}
          className="flex justify-between items-center p-3 bg-gray-700/50 rounded hover:bg-gray-700 transition-colors"
        >
          <div>
            <span className="text-sm">{opp.market_title}</span>
            <div className="flex items-center gap-2 mt-1">
              <span className="text-xs text-gray-400">
                {opp.platform_buy} <span className="text-gray-600">→</span> {opp.platform_sell}
              </span>
              {opp.is_risk_free && (
                <span className="text-xs px-1.5 py-0.5 bg-green-900/50 text-green-300 rounded">
                  Risk-Free
                </span>
              )}
            </div>
          </div>
          <div className="text-right">
            <span className="text-green-400 font-mono text-lg">{opp.edge_pct.toFixed(2)}%</span>
            <div className="text-xs text-gray-500">${opp.liquidity_buy?.toFixed(0) || '—'} liq</div>
          </div>
        </div>
      ))}
      {(!opportunities || opportunities.length === 0) && (
        <p className="text-gray-400 text-center py-8">No active opportunities</p>
      )}
    </div>
  )
}
