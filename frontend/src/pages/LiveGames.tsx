import { useQuery } from '@tanstack/react-query'
import { useWebSocket } from '../hooks/useWebSocket'

export default function LiveGames() {
  const { subscribe } = useWebSocket()

  const { data: games, isLoading } = useQuery({
    queryKey: ['liveGames'],
    queryFn: async () => {
      const res = await fetch('/api/live-games')
      return res.json()
    },
  })

  return (
    <div className="space-y-6 h-full flex flex-col">
      <div className="flex justify-between items-center">
        <h1 className="text-3xl font-bold">Live Games</h1>
        <span className="text-sm text-gray-400">{games?.length || 0} games</span>
      </div>

      <div className="flex-1 overflow-y-auto min-h-0">
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4 pb-4">
          {isLoading && <p className="text-gray-400">Loading...</p>}
          {games?.map((game: any) => (
            <GameCard key={game.game_id} game={game} onSubscribe={() => subscribe(game.game_id)} />
          ))}
          {(!isLoading && (!games || games.length === 0)) && (
            <p className="text-gray-400 col-span-full text-center py-8">No live games at the moment</p>
          )}
        </div>
      </div>
    </div>
  )
}

function GameCard({ game, onSubscribe }: { game: any; onSubscribe: () => void }) {
  const { data: state } = useQuery({
    queryKey: ['gameState', game.game_id],
    queryFn: async () => {
      const res = await fetch(`/api/live-games/${game.game_id}/state`)
      if (!res.ok) return null
      return res.json()
    },
    enabled: !!game.game_id,
  })

  return (
    <div className="bg-gray-800 rounded-lg p-4 hover:bg-gray-750 transition-colors">
      <div className="flex justify-between items-start mb-3">
        <span className="text-xs bg-gray-700 px-2 py-1 rounded uppercase">{game.sport}</span>
        <span className="text-xs text-gray-400">{game.status}</span>
      </div>

      <div className="space-y-2">
        {/* Away team listed first (standard sports format) */}
        <div className="flex justify-between items-center">
          <span className="truncate max-w-[180px] text-gray-300">{game.away_team || 'Away'}</span>
          <span className="text-2xl font-mono">{game.away_score}</span>
        </div>
        {/* Home team listed second */}
        <div className="flex justify-between items-center">
          <span className="truncate max-w-[180px]">{game.home_team || 'Home'}</span>
          <span className="text-2xl font-mono">{game.home_score}</span>
        </div>
      </div>

      {state && (
        <div className="mt-3 pt-3 border-t border-gray-700 text-sm text-gray-400">
          <div className="flex justify-between">
            <span>Q{state.period}</span>
            <span>{state.time_remaining}</span>
          </div>
          {state.home_win_prob && (
            <div className="mt-1">
              <span>Win prob: </span>
              <span className="text-green-400">{(state.home_win_prob * 100).toFixed(1)}%</span>
            </div>
          )}
        </div>
      )}

      <button
        onClick={onSubscribe}
        className="mt-3 w-full py-2 bg-gray-700 hover:bg-gray-600 rounded text-sm transition-colors"
      >
        Subscribe to Updates
      </button>
    </div>
  )
}
