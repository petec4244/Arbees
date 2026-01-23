import { useState, useMemo } from 'react'
import { Settings, BarChart2, Activity, TrendingUp, Filter } from 'lucide-react'
import PropChart, { ChartDataPoint, Trade } from './PropChart'
import CandlestickChart, { OHLCData } from './CandlestickChart'

interface GameTrackerProps {
    gameId?: string
    title?: string // e.g. "Moneyline" or "LeBron Points"
    homeTeam: string
    awayTeam: string
    history: Array<{ timestamp: number; homeValue: number; awayValue: number }> // Raw tick data
    trades?: Trade[]
}

const TIME_INTERVALS = [
    { label: '1m', value: 60 * 1000 },
    { label: '5m', value: 5 * 60 * 1000 },
    { label: '15m', value: 15 * 60 * 1000 },
]

export default function GameTracker({
    gameId,
    title = "Prop Tracker",
    homeTeam,
    awayTeam,
    history = [],
    trades = []
}: GameTrackerProps) {
    const [chartType, setChartType] = useState<'line' | 'candle'>('line')
    const [interval, setInterval] = useState(TIME_INTERVALS[0].value)
    const [showHome, setShowHome] = useState(true)
    const [showAway, setShowAway] = useState(true)

    // Process data for Line Chart
    // We just map the raw history to the format PropChart expects with formatted time
    const lineData: ChartDataPoint[] = useMemo(() => {
        return history.map(h => ({
            time: new Date(h.timestamp).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
            homeValue: h.homeValue,
            awayValue: h.awayValue,
            timestamp: h.timestamp
        }))
    }, [history])

    // Process data for Candlestick Chart
    // We need to aggregate raw ticks into OHLC bars based on the selected interval
    // Note: We can only show one Candle series at a time usually, or we need 2 charts.
    // For this implementation, let's assume if Candle is selected, we track the Home Team (or primary prop).
    // Or we could add a selector for "Candle Focus: Home | Away".
    const [candleFocus, setCandleFocus] = useState<'home' | 'away'>('home')

    const candleData: OHLCData[] = useMemo(() => {
        if (history.length === 0) return []

        const sorted = [...history].sort((a, b) => a.timestamp - b.timestamp)
        const bars: OHLCData[] = []

        let currentBar: Partial<OHLCData> | null = null
        let barStartTime = Math.floor(sorted[0].timestamp / interval) * interval

        sorted.forEach(tick => {
            const val = candleFocus === 'home' ? tick.homeValue : tick.awayValue
            const tickTime = tick.timestamp

            // If tick is outside current bar, close current and start new
            if (tickTime >= barStartTime + interval) {
                if (currentBar) {
                    bars.push(currentBar as OHLCData)
                }

                // Move to the next slot that contains this tick
                barStartTime = Math.floor(tickTime / interval) * interval
                currentBar = {
                    time: new Date(barStartTime).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
                    open: val,
                    high: val,
                    low: val,
                    close: val
                }
            } else {
                if (!currentBar) {
                    currentBar = {
                        time: new Date(barStartTime).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
                        open: val,
                        high: val,
                        low: val,
                        close: val
                    }
                } else {
                    currentBar.high = Math.max(currentBar.high!, val)
                    currentBar.low = Math.min(currentBar.low!, val)
                    currentBar.close = val
                }
            }
        })

        if (currentBar) {
            bars.push(currentBar as OHLCData)
        }

        return bars
    }, [history, interval, candleFocus])

    return (
        <div className="bg-gray-800 rounded-lg border border-gray-700 overflow-hidden">
            {/* Header Controls */}
            <div className="p-4 border-b border-gray-700 flex flex-col md:flex-row justify-between items-start md:items-center gap-4">
                <div>
                    <h3 className="text-lg font-semibold text-white flex items-center gap-2">
                        <Activity className="w-5 h-5 text-green-400" />
                        {title}
                    </h3>
                    <div className="flex items-center gap-2 text-xs text-gray-400 mt-1">
                        <span className="bg-gray-700 px-1.5 py-0.5 rounded">{homeTeam} vs {awayTeam}</span>
                        <span>•</span>
                        <span>{history.length} updates</span>
                    </div>
                </div>

                <div className="flex items-center gap-2 bg-gray-900/50 p-1.5 rounded-lg">
                    {/* Chart Type Toggle */}
                    <div className="flex bg-gray-800 rounded">
                        <button
                            onClick={() => setChartType('line')}
                            className={`p-1.5 rounded transition-colors ${chartType === 'line' ? 'bg-blue-600 text-white' : 'text-gray-400 hover:text-white'}`}
                            title="Line Chart"
                        >
                            <TrendingUp className="w-4 h-4" />
                        </button>
                        <button
                            onClick={() => setChartType('candle')}
                            className={`p-1.5 rounded transition-colors ${chartType === 'candle' ? 'bg-blue-600 text-white' : 'text-gray-400 hover:text-white'}`}
                            title="Candlestick Chart"
                        >
                            <BarChart2 className="w-4 h-4" />
                        </button>
                    </div>

                    <div className="h-4 w-px bg-gray-700 mx-1" />

                    {/* Series Toggles (Line Mode) */}
                    {chartType === 'line' && (
                        <div className="flex items-center gap-2">
                            <button
                                onClick={() => setShowHome(!showHome)}
                                className={`px-2 py-1 text-xs rounded border transition-colors ${showHome ? 'bg-green-900/30 border-green-500/50 text-green-400' : 'bg-transparent border-gray-700 text-gray-500'}`}
                            >
                                {homeTeam}
                            </button>
                            <button
                                onClick={() => setShowAway(!showAway)}
                                className={`px-2 py-1 text-xs rounded border transition-colors ${showAway ? 'bg-blue-900/30 border-blue-500/50 text-blue-400' : 'bg-transparent border-gray-700 text-gray-500'}`}
                            >
                                {awayTeam}
                            </button>
                        </div>
                    )}

                    {/* Series Selector (Candle Mode) */}
                    {chartType === 'candle' && (
                        <select
                            value={candleFocus}
                            onChange={(e) => setCandleFocus(e.target.value as 'home' | 'away')}
                            className="bg-gray-800 text-xs text-white border border-gray-700 rounded px-2 py-1 focus:outline-none"
                        >
                            <option value="home">{homeTeam}</option>
                            <option value="away">{awayTeam}</option>
                        </select>
                    )}

                    {chartType === 'candle' && (
                        <div className="h-4 w-px bg-gray-700 mx-1" />
                    )}

                    {/* Interval Selector (Candle Mode) */}
                    {chartType === 'candle' && (
                        <select
                            value={interval}
                            onChange={(e) => setInterval(Number(e.target.value))}
                            className="bg-gray-800 text-xs text-white border border-gray-700 rounded px-2 py-1 focus:outline-none"
                        >
                            {TIME_INTERVALS.map(t => (
                                <option key={t.value} value={t.value}>{t.label}</option>
                            ))}
                        </select>
                    )}
                </div>
            </div>

            {/* Chart Area */}
            <div className="p-4 bg-gray-900/30 min-h-[300px]">
                {chartType === 'line' ? (
                    <PropChart
                        data={lineData}
                        homeTeam={homeTeam}
                        awayTeam={awayTeam}
                        trades={trades}
                        showHome={showHome}
                        showAway={showAway}
                        height={300}
                    />
                ) : (
                    <CandlestickChart
                        data={candleData}
                        height={300}
                        upColor="#10B981"
                        downColor="#EF4444"
                    />
                )}
            </div>

            {/* Footer / Stats could go here */}
            {trades.length > 0 && (
                <div className="px-4 py-2 bg-gray-900/50 border-t border-gray-800 text-xs text-gray-400 flex items-center gap-4">
                    <span>Trades: {trades.length}</span>
                    <span>•</span>
                    <span className="text-green-400">Winning: {trades.filter(t => (t.pnl || 0) > 0).length}</span>
                    <span className="text-red-400">Losing: {trades.filter(t => (t.pnl || 0) < 0).length}</span>
                </div>
            )}
        </div>
    )
}
