import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, ReferenceLine } from 'recharts'

interface WinProbChartProps {
    data: Array<{ time: string; prob: number }>
    homeTeam: string
    awayTeam: string
}

export default function WinProbChart({ data, homeTeam, awayTeam }: WinProbChartProps) {
    if (!data || data.length === 0) {
        return (
            <div className="h-48 flex items-center justify-center bg-gray-900/50 rounded-lg text-gray-500">
                No probability data available
            </div>
        )
    }

    return (
        <div className="h-64 w-full">
            <ResponsiveContainer width="100%" height="100%">
                <LineChart data={data} margin={{ top: 5, right: 20, bottom: 5, left: 0 }}>
                    <CartesianGrid strokeDasharray="3 3" stroke="#374151" />
                    <XAxis
                        dataKey="time"
                        stroke="#9CA3AF"
                        fontSize={12}
                        tickLine={false}
                    />
                    <YAxis
                        domain={[0, 100]}
                        stroke="#9CA3AF"
                        fontSize={12}
                        tickFormatter={(value) => `${value}%`}
                        tickLine={false}
                    />
                    <Tooltip
                        contentStyle={{ backgroundColor: '#1F2937', border: '1px solid #374151', color: '#F3F4F6' }}
                        formatter={(value: number) => [`${value.toFixed(1)}%`, 'Home Win Prob']}
                    />
                    <ReferenceLine y={50} stroke="#4B5563" strokeDasharray="3 3" />
                    <Line
                        type="monotone"
                        dataKey="prob"
                        stroke="#10B981"
                        strokeWidth={2}
                        dot={false}
                        activeDot={{ r: 4 }}
                    />
                </LineChart>
            </ResponsiveContainer>
            <div className="flex justify-between px-4 text-xs text-gray-500">
                <span>{awayTeam}</span>
                <span>{homeTeam}</span>
            </div>
        </div>
    )
}
