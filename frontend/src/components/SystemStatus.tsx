import { useQuery } from '@tanstack/react-query'
import { Activity, Database, Server } from 'lucide-react'

export default function SystemStatus() {
    const { data: status, isError } = useQuery({
        queryKey: ['systemStatus'],
        queryFn: async () => {
            const start = performance.now()
            const res = await fetch('/api/monitoring/status')
            const end = performance.now()
            if (!res.ok) throw new Error('Status check failed')
            return { ...(await res.json()), ping: Math.round(end - start) }
        },
        refetchInterval: 10000,
    })

    // Mock data if backend endpoint doesn't exist yet
    const displayStatus = status || {
        redis: true,
        timescaledb: true,
        shards: 1,
        ping: 0
    }

    const isHealthy = !isError && displayStatus.redis && displayStatus.timescaledb

    return (
        <div className="flex items-center space-x-6 text-sm text-gray-400 bg-gray-900/50 px-4 py-2 rounded-full border border-gray-700/50 backdrop-blur-sm">
            <div className="flex items-center space-x-2" title="Redis Connection">
                <Server className={`w-4 h-4 ${displayStatus.redis ? 'text-green-400' : 'text-red-400'}`} />
                <span className="hidden lg:inline">Redis</span>
            </div>

            <div className="flex items-center space-x-2" title="Database Connection">
                <Database className={`w-4 h-4 ${displayStatus.timescaledb ? 'text-green-400' : 'text-red-400'}`} />
                <span className="hidden lg:inline">DB</span>
            </div>

            <div className="flex items-center space-x-2" title="Active GameShards">
                <Activity className={`w-4 h-4 ${displayStatus.shards > 0 ? 'text-blue-400' : 'text-yellow-400'}`} />
                <span className="hidden lg:inline">{displayStatus.shards} Shards</span>
            </div>

            <div className="flex items-center space-x-2 w-24 justify-end font-mono" title="API Round-trip Time">
                <span className="text-xs text-gray-500 mr-1">PING</span>
                <span className={`${displayStatus.ping > 500 ? 'text-yellow-400' : 'text-gray-400'}`}>
                    {displayStatus.ping}ms
                </span>
            </div>
        </div>
    )
}
