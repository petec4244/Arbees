import { useState, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import { Clock, Filter, Calendar, MapPin, Tv, ChevronDown, ChevronUp, AlertCircle, Timer, CalendarClock, LineChart, ArrowUpDown, Activity } from 'lucide-react'
import { getMarketConfig, MarketBackground } from '../utils/board_config'

interface UpcomingMarket {
  game_id: string
  sport: string // used as market type
  home_team: string
  away_team: string
  home_team_abbrev?: string
  away_team_abbrev?: string
  scheduled_time: string
  venue?: string
  broadcast?: string
  time_category: 'imminent' | 'soon' | 'upcoming' | 'future'
  time_until_start: string
  minutes_until_start: number
  metadata?: any
}

interface UpcomingMarketsStats {
  total_games: number
  by_category: {
    imminent: number
    soon: number
    upcoming: number
    future: number
  }
  by_sport: Record<string, number>
}

const TIME_CATEGORY_CONFIG = {
  imminent: {
    label: 'Starting Soon',
    color: 'text-red-400',
    bgColor: 'bg-red-500/10',
    borderColor: 'border-red-500/30',
    icon: AlertCircle,
    description: 'Less than 30 minutes',
  },
  soon: {
    label: 'Starting Shortly',
    color: 'text-yellow-400',
    bgColor: 'bg-yellow-500/10',
    borderColor: 'border-yellow-500/30',
    icon: Timer,
    description: '30 min - 2 hours',
  },
  upcoming: {
    label: 'Today',
    color: 'text-blue-400',
    bgColor: 'bg-blue-500/10',
    borderColor: 'border-blue-500/30',
    icon: Clock,
    description: '2 - 24 hours',
  },
  future: {
    label: 'Scheduled',
    color: 'text-gray-400',
    bgColor: 'bg-gray-500/10',
    borderColor: 'border-gray-500/30',
    icon: CalendarClock,
    description: 'More than 24 hours',
  },
}

const HOURS_OPTIONS = [
  { value: 6, label: '6 hours' },
  { value: 12, label: '12 hours' },
  { value: 24, label: '24 hours' },
  { value: 48, label: '2 days' },
  { value: 168, label: '1 week' },
]

function StartedMarketsList({ markets }: { markets: any[] }) {
  if (!markets || markets.length === 0) return null

  return (
    <div className="bg-gradient-to-r from-red-900/10 to-transparent border-l-4 border-red-500/50 bg-gray-900/30 rounded-lg p-4 mb-6">
      <div className="flex items-center gap-2 mb-3">
        <Activity className="w-5 h-5 text-red-500 animate-pulse" />
        <h2 className="text-lg font-bold text-gray-100">Live Markets</h2>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-3">
        {markets.map(market => {
          // Helper to check if it's a sport or other market type
          const isSport = !['crypto', 'economics', 'politics'].includes(market.sport?.toLowerCase());

          return (
            <div key={market.game_id} className="bg-gray-800/80 rounded border border-gray-700 p-3 flex justify-between items-center group hover:bg-gray-800 transition-colors">
              <div className="flex flex-col gap-1 flex-1">
                <div className="flex justify-between items-center w-full">
                  <span className="text-gray-300 text-sm font-medium">{market.away_team_abbrev || market.away_team}</span>
                  {isSport && <span className={`text-lg font-mono font-bold ${market.away_score > market.home_score ? 'text-white' : 'text-gray-500'}`}>{market.away_score}</span>}
                </div>
                <div className="flex justify-between items-center w-full">
                  <span className="text-gray-300 text-sm font-medium">{market.home_team_abbrev || market.home_team}</span>
                  {isSport && <span className={`text-lg font-mono font-bold ${market.home_score > market.away_score ? 'text-white' : 'text-gray-500'}`}>{market.home_score}</span>}
                </div>
              </div>

              <div className="border-l border-gray-700 pl-3 ml-3 flex flex-col items-end min-w-[60px]">
                <span className="text-[10px] uppercase text-gray-500 font-bold mb-1">{market.sport}</span>
                {market.status?.toLowerCase().includes('final') || market.status?.toLowerCase().includes('complete') ? (
                  <span className="text-xs bg-gray-700 text-gray-300 px-1.5 py-0.5 rounded font-medium">FINAL</span>
                ) : (
                  <>
                    <span className="text-xs text-green-400 font-bold animate-pulse">LIVE</span>
                    {isSport && <span className="text-[10px] text-gray-400">{market.period ? `Q${market.period}` : ''} {market.time_remaining}</span>}
                  </>
                )}
              </div>
            </div>
          )
        })}
      </div>
    </div>
  )
}

export default function UpcomingGames() {
  const [selectedMarketType, setSelectedMarketType] = useState<string>('ALL')
  const [hoursAhead, setHoursAhead] = useState<number>(24)
  const [collapsedCategories, setCollapsedCategories] = useState<Set<string>>(new Set())
  const [sortBy, setSortBy] = useState<'time' | 'market'>('time')

  // Fetch live markets
  const { data: startedMarkets } = useQuery({
    queryKey: ['startedMarkets', selectedMarketType],
    queryFn: async () => {
      let url = '/api/live-games?include_final=true&max_age_hours=3'
      if (selectedMarketType !== 'ALL') {
        url += `&sport=${selectedMarketType}`
      }
      const res = await fetch(url)
      if (!res.ok) return []
      return res.json()
    },
    refetchInterval: 10000,
  })

  // Fetch upcoming markets
  const { data: markets, isLoading, isError } = useQuery<UpcomingMarket[]>({
    queryKey: ['upcomingMarkets', hoursAhead, selectedMarketType === 'ALL' ? undefined : selectedMarketType],
    queryFn: async () => {
      const params = new URLSearchParams({
        hours_ahead: hoursAhead.toString(),
        limit: '100',
      })
      if (selectedMarketType !== 'ALL') {
        params.append('sport', selectedMarketType.toLowerCase())
      }
      const res = await fetch(`/api/upcoming-games?${params}`)
      if (!res.ok) throw new Error('Failed to fetch upcoming markets')
      return res.json()
    },
    refetchInterval: 60000,
  })

  // Fetch stats separately if needed, or derived from markets
  const { data: stats } = useQuery<UpcomingMarketsStats>({
    queryKey: ['upcomingMarketsStats', hoursAhead],
    queryFn: async () => {
      const res = await fetch(`/api/upcoming-games/stats?hours_ahead=${hoursAhead}`)
      if (!res.ok) throw new Error('Failed to fetch stats')
      return res.json()
    },
    refetchInterval: 60000,
  })

  // Fetch monitored IDs
  const { data: futuresGames } = useQuery<{ game_id: string }[]>({
    queryKey: ['futuresGamesIds'],
    queryFn: async () => {
      const res = await fetch('/api/futures/games?limit=100')
      if (!res.ok) return []
      return res.json()
    },
    refetchInterval: 60000,
  })

  const futuresGameIds = useMemo(() => {
    return new Set(futuresGames?.map(g => g.game_id) || [])
  }, [futuresGames])

  // Get unique market types
  const marketTypes = useMemo(() => {
    if (stats?.by_sport) {
      return ['ALL', ...Object.keys(stats.by_sport).sort()]
    }
    if (markets) {
      const distinct = new Set(markets.map(g => g.sport))
      return ['ALL', ...Array.from(distinct).sort()]
    }
    return ['ALL']
  }, [markets, stats])

  // Group markets
  const groupedMarkets = useMemo(() => {
    if (!markets) return {}

    let sorted = [...markets]
    if (sortBy === 'market') {
      sorted.sort((a, b) => {
        const typeDiff = a.sport.localeCompare(b.sport)
        if (typeDiff !== 0) return typeDiff
        return new Date(a.scheduled_time).getTime() - new Date(b.scheduled_time).getTime()
      })
    } else {
      sorted.sort((a, b) => new Date(a.scheduled_time).getTime() - new Date(b.scheduled_time).getTime())
    }

    const groups: Record<string, UpcomingMarket[]> = {
      imminent: [],
      soon: [],
      upcoming: [],
      future: [],
    }

    sorted.forEach(m => {
      const category = m.time_category || 'upcoming'
      if (groups[category]) {
        groups[category].push(m)
      }
    })

    return groups
  }, [markets, sortBy])

  const toggleCategory = (category: string) => {
    setCollapsedCategories(prev => {
      const next = new Set(prev)
      if (next.has(category)) {
        next.delete(category)
      } else {
        next.add(category)
      }
      return next
    })
  }

  return (
    <div className="space-y-6 h-full flex flex-col">
      {/* Header */}
      <div className="flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
        <div>
          <h1 className="text-3xl font-bold flex items-center gap-3">
            <Calendar className="w-8 h-8 text-blue-400" />
            Market Schedule
          </h1>
          <span className="text-sm text-gray-400">
            {markets?.length || 0} events in the next {hoursAhead} hours
          </span>
        </div>

        {/* Filters */}
        <div className="flex items-center space-x-3 bg-gray-800 p-2 rounded-lg">
          {/* Market Type Filter */}
          <div className="flex items-center space-x-2 px-2">
            <Filter className="w-4 h-4 text-gray-400" />
            <select
              value={selectedMarketType}
              onChange={(e) => setSelectedMarketType(e.target.value)}
              className="bg-transparent text-sm focus:outline-none cursor-pointer"
            >
              {marketTypes.map((s) => (
                <option key={s} value={s} className="bg-gray-800">
                  {s.toUpperCase()} {stats?.by_sport[s] ? `(${stats.by_sport[s]})` : ''}
                </option>
              ))}
            </select>
          </div>

          <div className="w-px h-4 bg-gray-700" />

          {/* Time Window */}
          <div className="flex items-center space-x-2 px-2">
            <Clock className="w-4 h-4 text-gray-400" />
            <select
              value={hoursAhead}
              onChange={(e) => setHoursAhead(Number(e.target.value))}
              className="bg-transparent text-sm focus:outline-none cursor-pointer"
            >
              {HOURS_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value} className="bg-gray-800">
                  {opt.label}
                </option>
              ))}
            </select>
          </div>

          <div className="w-px h-4 bg-gray-700" />

          {/* Sort Control */}
          <button
            onClick={() => setSortBy(prev => prev === 'time' ? 'market' : 'time')}
            className="flex items-center space-x-2 px-2 text-gray-400 hover:text-white transition-colors"
          >
            <ArrowUpDown className="w-4 h-4" />
            <span className="text-sm">{sortBy === 'time' ? 'Time' : 'Type'}</span>
          </button>
        </div>
      </div>

      {/* Started/Live Markets */}
      <StartedMarketsList markets={startedMarkets} />

      {/* Stats Summary */}
      {stats && (
        <div className="grid grid-cols-2 md:grid-cols-4 gap-3">
          {Object.entries(TIME_CATEGORY_CONFIG).map(([key, config]) => {
            const count = stats.by_category[key as keyof typeof stats.by_category] || 0
            const Icon = config.icon
            return (
              <div
                key={key}
                className={`${config.bgColor} ${config.borderColor} border rounded-lg p-3 flex items-center gap-3`}
              >
                <Icon className={`w-5 h-5 ${config.color}`} />
                <div>
                  <div className={`text-lg font-bold ${config.color}`}>{count}</div>
                  <div className="text-xs text-gray-400">{config.label}</div>
                </div>
              </div>
            )
          })}
        </div>
      )}

      {/* Markets List */}
      <div className="flex-1 overflow-y-auto min-h-0 pr-2 custom-scrollbar space-y-4 pb-4">
        {isLoading && (
          <div className="flex items-center justify-center p-12 text-gray-400">
            <div className="animate-spin w-6 h-6 border-2 border-blue-400 border-t-transparent rounded-full mr-3" />
            Loading markets...
          </div>
        )}

        {isError && (
          <div className="flex flex-col items-center justify-center p-12 text-red-400 bg-red-500/10 rounded-lg border border-red-500/30">
            <AlertCircle className="w-8 h-8 mb-2" />
            <p>Failed to load markets</p>
          </div>
        )}

        {!isLoading && !isError && markets?.length === 0 && (
          <div className="flex flex-col items-center justify-center p-12 text-gray-500 bg-gray-800/50 rounded-lg border border-gray-700 border-dashed">
            <Calendar className="w-12 h-12 mb-4 opacity-20" />
            <p>No events in the next {hoursAhead} hours</p>
          </div>
        )}

        {!isLoading && !isError && Object.entries(groupedMarkets).map(([category, categoryMarkets]) => {
          if (categoryMarkets.length === 0) return null

          const config = TIME_CATEGORY_CONFIG[category as keyof typeof TIME_CATEGORY_CONFIG]
          const Icon = config.icon
          const isCollapsed = collapsedCategories.has(category)

          return (
            <div key={category} className={`${config.bgColor} ${config.borderColor} border rounded-lg overflow-hidden`}>
              {/* Category Header */}
              <button
                onClick={() => toggleCategory(category)}
                className="w-full flex items-center justify-between p-4 hover:bg-white/5 transition-colors"
              >
                <div className="flex items-center gap-3">
                  <Icon className={`w-5 h-5 ${config.color}`} />
                  <div className="text-left">
                    <div className={`font-bold ${config.color}`}>{config.label}</div>
                    <div className="text-xs text-gray-400">{config.description}</div>
                  </div>
                  <span className={`ml-2 px-2 py-0.5 rounded-full text-xs font-bold ${config.bgColor} ${config.color}`}>
                    {categoryMarkets.length}
                  </span>
                </div>
                {isCollapsed ? (
                  <ChevronDown className="w-5 h-5 text-gray-400" />
                ) : (
                  <ChevronUp className="w-5 h-5 text-gray-400" />
                )}
              </button>

              {/* Markets Grid */}
              {!isCollapsed && (
                <div className="p-4 pt-0 grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-3">
                  {categoryMarkets.map((market) => (
                    <MarketCard
                      key={market.game_id}
                      market={market}
                      isMonitoredByFutures={futuresGameIds.has(market.game_id)}
                    />
                  ))}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}

function MarketCard({ market, isMonitoredByFutures }: { market: UpcomingMarket; isMonitoredByFutures?: boolean }) {
  const config = TIME_CATEGORY_CONFIG[market.time_category]
  const scheduledDate = new Date(market.scheduled_time)
  const marketConfig = getMarketConfig(market.sport)

  return (
    <div className={`rounded-lg overflow-hidden relative group border ${marketConfig.colors} transition-colors`}>
      <MarketBackground type={market.sport} />
      <div className="p-4 relative z-10">
        {/* Header */}
        <div className="flex justify-between items-start mb-3">
          <div className="flex items-center gap-2">
            <span className={`text-xs font-bold px-2 py-0.5 rounded uppercase border ${marketConfig.badge}`}>
              {market.sport}
            </span>
            {isMonitoredByFutures && (
              <Link
                to="/futures"
                className="flex items-center gap-1 text-xs font-bold bg-purple-900/50 border border-purple-500/30 px-2 py-0.5 rounded text-purple-300 hover:bg-purple-900/70 transition-colors"
                title="Being monitored by Futures"
              >
                <LineChart className="w-3 h-3" />
                <span>F</span>
              </Link>
            )}
          </div>
          <span className={`text-xs font-bold px-2 py-0.5 rounded ${config.bgColor} ${config.color}`}>
            {market.time_until_start}
          </span>
        </div>

        {/* Teams/Entities */}
        <div className="space-y-2 mb-3">
          <div className="flex justify-between items-center">
            <span className="text-gray-300 font-medium truncate max-w-[80%]">
              {market.away_team_abbrev || market.away_team}
            </span>
            <span className="text-xs text-gray-500">NO / AWAY</span>
          </div>
          <div className="flex justify-between items-center">
            <span className="text-white font-medium truncate max-w-[80%]">
              {market.home_team_abbrev || market.home_team}
            </span>
            <span className="text-xs text-gray-500">YES / HOME</span>
          </div>
        </div>

        {/* Market Time */}
        <div className="border-t border-gray-700 pt-3 space-y-1">
          <div className="flex items-center gap-2 text-xs text-gray-400">
            <Clock className="w-3 h-3" />
            <span>{scheduledDate.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
            <span className="text-gray-600">|</span>
            <span>{scheduledDate.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })}</span>
          </div>

          {market.venue && (
            <div className="flex items-center gap-2 text-xs text-gray-500">
              <MapPin className="w-3 h-3" />
              <span className="truncate">{market.venue}</span>
            </div>
          )}

          {market.broadcast && (
            <div className="flex items-center gap-2 text-xs text-gray-500">
              <Tv className="w-3 h-3" />
              <span>{market.broadcast}</span>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
