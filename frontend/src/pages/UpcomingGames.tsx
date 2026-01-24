import { useState, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import { Clock, Filter, Calendar, MapPin, Tv, ChevronDown, ChevronUp, AlertCircle, Timer, CalendarClock, LineChart, ArrowUpDown } from 'lucide-react'
import { getSportConfig, SportBackground } from '../utils/sports'

interface UpcomingGame {
  game_id: string
  sport: string
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
}

interface UpcomingGamesStats {
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

export default function UpcomingGames() {
  const [selectedSport, setSelectedSport] = useState<string>('ALL')
  const [hoursAhead, setHoursAhead] = useState<number>(24)
  const [collapsedCategories, setCollapsedCategories] = useState<Set<string>>(new Set())
  const [sortBy, setSortBy] = useState<'time' | 'sport'>('time')

  const { data: games, isLoading, isError } = useQuery<UpcomingGame[]>({
    queryKey: ['upcomingGames', hoursAhead, selectedSport === 'ALL' ? undefined : selectedSport],
    queryFn: async () => {
      const params = new URLSearchParams({
        hours_ahead: hoursAhead.toString(),
        limit: '100',
      })
      if (selectedSport !== 'ALL') {
        params.append('sport', selectedSport.toLowerCase())
      }
      const res = await fetch(`/api/upcoming-games?${params}`)
      if (!res.ok) throw new Error('Failed to fetch upcoming games')
      return res.json()
    },
    refetchInterval: 60000, // Refresh every minute
  })

  const { data: stats } = useQuery<UpcomingGamesStats>({
    queryKey: ['upcomingGamesStats', hoursAhead],
    queryFn: async () => {
      const res = await fetch(`/api/upcoming-games/stats?hours_ahead=${hoursAhead}`)
      if (!res.ok) throw new Error('Failed to fetch stats')
      return res.json()
    },
    refetchInterval: 60000,
  })

  // Fetch futures games to identify which are being monitored
  const { data: futuresGames } = useQuery<{ game_id: string }[]>({
    queryKey: ['futuresGamesIds'],
    queryFn: async () => {
      const res = await fetch('/api/futures/games?limit=100')
      if (!res.ok) return []
      return res.json()
    },
    refetchInterval: 60000,
  })

  // Set of game IDs being monitored by futures
  const futuresGameIds = useMemo(() => {
    return new Set(futuresGames?.map(g => g.game_id) || [])
  }, [futuresGames])

  // Get unique sports from games or stats
  const sports = useMemo(() => {
    if (stats?.by_sport) {
      return ['ALL', ...Object.keys(stats.by_sport).sort()]
    }
    if (games) {
      const distinct = new Set(games.map(g => g.sport))
      return ['ALL', ...Array.from(distinct).sort()]
    }
    return ['ALL']
  }, [games, stats])

  // Group games by time category (or maintain flat list if sorting by sport)
  const groupedGames = useMemo(() => {
    if (!games) return {}

    // Clone and sort games first
    let sortedGames = [...games]
    if (sortBy === 'sport') {
      sortedGames.sort((a, b) => {
        // Sort by sport, then by time
        const sportDiff = a.sport.localeCompare(b.sport)
        if (sportDiff !== 0) return sportDiff
        return new Date(a.scheduled_time).getTime() - new Date(b.scheduled_time).getTime()
      })
    } else {
      // Sort by time
      sortedGames.sort((a, b) => new Date(a.scheduled_time).getTime() - new Date(b.scheduled_time).getTime())
    }

    // If sorting by sport, we might want to group differently or just respect the grouping
    // Current design is grouping by time category. Let's keep that but sorting inside categories?
    // User asked "Sort by Sport". If we group by "Starting Soon", "Today" etc, sorting by sport INSIDE those makes sense.
    // If user explicitly wants to just see all NBA games together regardless of time, that's different.
    // Let's assume sorting inside the time groups for now to maintain layout structure.

    const groups: Record<string, UpcomingGame[]> = {
      imminent: [],
      soon: [],
      upcoming: [],
      future: [],
    }

    sortedGames.forEach(game => {
      const category = game.time_category || 'upcoming'
      if (groups[category]) {
        groups[category].push(game)
      }
    })

    return groups
  }, [games, sortBy])

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
            Upcoming Games
          </h1>
          <span className="text-sm text-gray-400">
            {games?.length || 0} games in the next {hoursAhead} hours
          </span>
        </div>

        {/* Filters */}
        <div className="flex items-center space-x-3 bg-gray-800 p-2 rounded-lg">
          {/* Sport Filter */}
          <div className="flex items-center space-x-2 px-2">
            <Filter className="w-4 h-4 text-gray-400" />
            <select
              value={selectedSport}
              onChange={(e) => setSelectedSport(e.target.value)}
              className="bg-transparent text-sm focus:outline-none cursor-pointer"
            >
              {sports.map((s) => (
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
            onClick={() => setSortBy(prev => prev === 'time' ? 'sport' : 'time')}
            className="flex items-center space-x-2 px-2 text-gray-400 hover:text-white transition-colors"
          >
            <ArrowUpDown className="w-4 h-4" />
            <span className="text-sm">{sortBy === 'time' ? 'Time' : 'Sport'}</span>
          </button>
        </div>
      </div>

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

      {/* Games List */}
      <div className="flex-1 overflow-y-auto min-h-0 pr-2 custom-scrollbar space-y-4 pb-4">
        {isLoading && (
          <div className="flex items-center justify-center p-12 text-gray-400">
            <div className="animate-spin w-6 h-6 border-2 border-blue-400 border-t-transparent rounded-full mr-3" />
            Loading upcoming games...
          </div>
        )}

        {isError && (
          <div className="flex flex-col items-center justify-center p-12 text-red-400 bg-red-500/10 rounded-lg border border-red-500/30">
            <AlertCircle className="w-8 h-8 mb-2" />
            <p>Failed to load upcoming games</p>
          </div>
        )}

        {!isLoading && !isError && games?.length === 0 && (
          <div className="flex flex-col items-center justify-center p-12 text-gray-500 bg-gray-800/50 rounded-lg border border-gray-700 border-dashed">
            <Calendar className="w-12 h-12 mb-4 opacity-20" />
            <p>No upcoming games in the next {hoursAhead} hours</p>
          </div>
        )}

        {!isLoading && !isError && Object.entries(groupedGames).map(([category, categoryGames]) => {
          if (categoryGames.length === 0) return null

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
                    {categoryGames.length}
                  </span>
                </div>
                {isCollapsed ? (
                  <ChevronDown className="w-5 h-5 text-gray-400" />
                ) : (
                  <ChevronUp className="w-5 h-5 text-gray-400" />
                )}
              </button>

              {/* Games Grid */}
              {!isCollapsed && (
                <div className="p-4 pt-0 grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-3">
                  {categoryGames.map((game) => (
                    <GameCard
                      key={game.game_id}
                      game={game}
                      isMonitoredByFutures={futuresGameIds.has(game.game_id)}
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

function GameCard({ game, isMonitoredByFutures }: { game: UpcomingGame; isMonitoredByFutures?: boolean }) {
  const config = TIME_CATEGORY_CONFIG[game.time_category]
  const scheduledDate = new Date(game.scheduled_time)
  const sportConfig = getSportConfig(game.sport)

  return (
    <div className={`rounded-lg overflow-hidden relative group border ${sportConfig.colors} transition-colors`}>
      <SportBackground sport={game.sport} />
      <div className="p-4 relative z-10">
        {/* Header */}
        <div className="flex justify-between items-start mb-3">
          <div className="flex items-center gap-2">
            <span className={`text-xs font-bold px-2 py-0.5 rounded uppercase border ${sportConfig.badge}`}>
              {game.sport}
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
            {game.time_until_start}
          </span>
        </div>

        {/* Teams */}
        <div className="space-y-2 mb-3">
          <div className="flex justify-between items-center">
            <span className="text-gray-300 font-medium truncate max-w-[80%]">
              {game.away_team_abbrev || game.away_team}
            </span>
            <span className="text-xs text-gray-500">AWAY</span>
          </div>
          <div className="flex justify-between items-center">
            <span className="text-white font-medium truncate max-w-[80%]">
              {game.home_team_abbrev || game.home_team}
            </span>
            <span className="text-xs text-gray-500">HOME</span>
          </div>
        </div>

        {/* Game Time */}
        <div className="border-t border-gray-700 pt-3 space-y-1">
          <div className="flex items-center gap-2 text-xs text-gray-400">
            <Clock className="w-3 h-3" />
            <span>{scheduledDate.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
            <span className="text-gray-600">|</span>
            <span>{scheduledDate.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })}</span>
          </div>

          {game.venue && (
            <div className="flex items-center gap-2 text-xs text-gray-500">
              <MapPin className="w-3 h-3" />
              <span className="truncate">{game.venue}</span>
            </div>
          )}

          {game.broadcast && (
            <div className="flex items-center gap-2 text-xs text-gray-500">
              <Tv className="w-3 h-3" />
              <span>{game.broadcast}</span>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
