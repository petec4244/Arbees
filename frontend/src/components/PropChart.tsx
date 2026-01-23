import { LineChart, Line, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, ReferenceLine, ReferenceDot, Legend } from 'recharts'
import { useMemo } from 'react'

export interface Trade {
    id: string
    entryTime: string // ISO string
    exitTime?: string
    entryPrice: number
    exitPrice?: number
    side: 'buy' | 'sell'
    pnl?: number
    team?: 'home' | 'away'
}

export interface ChartDataPoint {
    time: string // ISO string or timestamp
    homeValue?: number
    awayValue?: number
    timestamp: number // Normalized timestamp for finding trades
}

interface PropChartProps {
    data: ChartDataPoint[]
    homeTeam: string
    awayTeam: string
    trades?: Trade[]
    showHome?: boolean
    showAway?: boolean
    width?: string | number
    height?: string | number
}

const formatTime = (time: string | number) => {
    return new Date(time).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

export default function PropChart({
    data,
    homeTeam,
    awayTeam,
    trades = [],
    showHome = true,
    showAway = true,
    width = "100%",
    height = 400
}: PropChartProps) {
    if (!data || data.length === 0) {
        return (
            <div className={`flex items-center justify-center bg-gray-900/50 rounded-lg text-gray-500 border border-gray-800`} style={{ height }}>
                No data available
            </div>
        )
    }

    // Process trades to map them to chart coordinates
    // We'll overlay them using ReferenceDot or ReferenceLine
    // However, ReferenceDot requires exact x/y matches or close proximity.
    // A better approach might be to add trade markers as a separate scatter series or use ReferenceDot with specific logic.
    // Creating a CustomDot might be cleaner.

    const renderTradeMarkers = () => {
        return trades.map((trade) => {
            // Find closest data point for x-axis positioning
            // For now assuming exact match or close enough, or Recharts handles time scale if type='number'
            // But our XAxis is likely category or time.
            // Let's stick to using the time string if it matches, or closest.

            // To simplify, we rely on the chart using a time scale or categorical with matching labels.
            // If data uses specific timestamps, we need to match them.

            const tradeTime = new Date(trade.entryTime).getTime()

            // Just returning ReferenceDots for now.
            // Note: Recharts ReferenceDot x value must match an x-axis value if axis type is category.
            // If axis type is number (timestamp), we can just pass the timestamp.

            // We'll use the trade properties to style the dot.
            // Color by team as requested (Home: Green, Away: Blue)
            const color = trade.team === 'home' ? '#34D399' : '#60A5FA';

            // We need to determine which Y value (home or away line) to attach to.
            // This requires knowing which team the trade was on.
            const yValue = trade.team === 'home' ? trade.entryPrice * 100 : trade.entryPrice * 100; // Assuming price is 0-1 and chart is 0-100

            const markers = [
                <ReferenceDot
                    key={`entry-${trade.id}`}
                    x={formatTime(trade.entryTime)}
                    y={yValue}
                    r={6}
                    fill={color}
                    stroke="#fff"
                    strokeWidth={2}
                    alwaysShow
                />
            ];

            if (trade.exitTime && trade.exitPrice) {
                const exitY = trade.exitPrice * 100; // Assuming same scale
                markers.push(
                    <ReferenceDot
                        key={`exit-${trade.id}`}
                        x={formatTime(trade.exitTime)}
                        y={exitY}
                        r={4}
                        fill="#fff"
                        stroke={color}
                        strokeWidth={2}
                        alwaysShow
                    />
                );
            }

            return markers;
        })
    }

    // Customized Tooltip
    const CustomTooltip = ({ active, payload, label }: any) => {
        if (active && payload && payload.length) {
            return (
                <div className="bg-gray-800 border border-gray-700 p-2 rounded shadow-lg text-xs">
                    <p className="font-bold text-gray-300 mb-1">{label}</p>
                    {payload.map((entry: any, index: number) => (
                        <div key={index} className="flex items-center gap-2">
                            <span className="w-2 h-2 rounded-full" style={{ backgroundColor: entry.color }}></span>
                            <span className="text-gray-400">{entry.name}:</span>
                            <span className="font-mono text-white">{entry.value.toFixed(1)}%</span>
                        </div>
                    ))}
                </div>
            )
        }
        return null
    }

    return (
        <div className="w-full relative" style={{ height }}>
            <ResponsiveContainer width={width} height="100%">
                <LineChart data={data} margin={{ top: 10, right: 30, left: 0, bottom: 0 }}>
                    <CartesianGrid strokeDasharray="3 3" stroke="#374151" vertical={false} />
                    <XAxis
                        dataKey="time"
                        stroke="#9CA3AF"
                        fontSize={11}
                        tickLine={false}
                        axisLine={false}
                        minTickGap={30}
                    />
                    <YAxis
                        stroke="#9CA3AF"
                        fontSize={11}
                        tickLine={false}
                        axisLine={false}
                        domain={[0, 100]} // Assuming probability/prop 0-100
                        tickFormatter={(value) => `${value}`}
                    />
                    <Tooltip content={<CustomTooltip />} />
                    <Legend iconType="circle" />
                    <ReferenceLine y={50} stroke="#4B5563" strokeDasharray="3 3" />

                    {showAway && (
                        <Line
                            type="monotone"
                            dataKey="awayValue"
                            name={awayTeam}
                            stroke="#60A5FA" // Blue for away
                            strokeWidth={2}
                            dot={false}
                            activeDot={{ r: 5 }}
                            animationDuration={300}
                        />
                    )}

                    {showHome && (
                        <Line
                            type="monotone"
                            dataKey="homeValue"
                            name={homeTeam}
                            stroke="#34D399" // Green for home
                            strokeWidth={2}
                            dot={false}
                            activeDot={{ r: 5 }}
                            animationDuration={300}
                        />
                    )}

                    {/* Trade Markers */}
                    {renderTradeMarkers()}
                </LineChart>
            </ResponsiveContainer>
        </div>
    )
}
