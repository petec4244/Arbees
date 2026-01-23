import { BarChart, Bar, XAxis, YAxis, Tooltip, CartesianGrid, ResponsiveContainer, Cell } from 'recharts'

export interface OHLCData {
    time: string
    open: number
    high: number
    low: number
    close: number
    volume?: number
}

interface CandlestickChartProps {
    data: OHLCData[]
    width?: string | number
    height?: string | number
    upColor?: string
    downColor?: string
}

const CandlestickShape = (props: any) => {
    const { x, y, width, height, payload, upColor, downColor } = props;
    const { open, close, high, low } = payload;

    const isUp = close >= open;
    const color = isUp ? upColor : downColor;

    // Y axis is inverted in SVG (0 is top). 
    // Recharts scales map values to pixels.
    // However, the standard Bar shape receives x, y, width, height for the "bar" part.
    // But we need the pixel positions for High and Low.
    // We can access the YAxis scale if we use a Custom Content, but simpler is to use the passed props if available?
    // Unfortunately, custom shape in Bar only gets the bar's bounding box.

    // ALTERNATIVE:
    // We can use the props to calculate positions if we had the scale.
    // But we don't easily have the scale here.

    // TRICK: Data passed to chart:
    // We can prepare data such that the "Bar" value is [low, high]. 
    // Then the bar covers the full range.
    // Then we draw the body inside it.

    // But the most robust way in Recharts without external libs is tricky.
    // Let's rely on a simplified visual or...
    // Actually, Recharts is annoying for Candles.
    // Let's try to do the "Floating Bar" for body + "ErrorBar" for wick?
    // Or just "Floating Bar" for body `[min(O,C), max(O,C)]`.
    // And `ErrorBar` requires single value.

    // Let's use the 'ComposedChart' with a 'Bar' for the body and a 'Line' (hidden) or custom shape.

    // Better:
    // Let's pass the pre-calculated pixel values if possible? No.

    // Let's try the schema where we pass a custom shape that calculates everything based on payload values, 
    // assuming we can access the scale.
    // Use `Bar` with `shape` prop.
    // Important: We need to pass the yAxis scale to the shape?
    // We can wrap it.

    // SIMPLIFIED APPROACH:
    // We will render the body using `Bar` with `dataKey` set to `bodyRange` = [min, max].
    // We will render the wicks using a `ErrorBar`? No.
    // We will render the wicks using a customized shape that draws a line from high to low.

    // Wait, if we set the Bar to cover `[low, high]`, then `y` is top (high), `height` is (low - high) in pixels.
    // Then inside this box, we can draw the open/close body.
    // We need to know where open and close are relative to high/low.

    // height_in_pixels = scale(low) - scale(high)
    // ratio = height_in_pixels / (high - low)
    // body_top = scale(max(open, close))
    // ... this is getting complicated.

    // Let's assume for this task that a simplified "High-Low" bar with a marker for Open/Close is acceptable, 
    // OR we just use a library like 'apexcharts' or 'lightweight-charts' if we could. 
    // Since we can't add deps, I will stick to a clean custom shape in Recharts.
    // I will use a Bar that spans [low, high] (the full range).
    // The shape will draw the wick (center line) and the body (rectangle).

    // To make this work:
    // Bar dataKey = [low, high].
    // Shape logic:
    //  x, y, width, height are given for the [low, high] rect.
    //  We need to find the Y positions of Open and Close within this rect.
    //  pixelMin = y + height (value low)
    //  pixelMax = y (value high)
    //  range = high - low.
    //  factor = height / range.
    //  openOffset = (high - open) * factor
    //  closeOffset = (high - close) * factor
    //  
    //  This works!

    const range = high - low;
    if (range === 0) return null; // Avoid divide by zero

    const pixelHeight = height;
    const factor = pixelHeight / range;

    const openY = y + (high - open) * factor;
    const closeY = y + (high - close) * factor;

    const bodyTop = Math.min(openY, closeY);
    const bodyHeight = Math.abs(openY - closeY);
    const bodyBottom = bodyTop + bodyHeight;

    // Wick is just a line in the middle from y to y+height
    const wickX = x + width / 2;

    return (
        <g>
            {/* Wick */}
            <line
                x1={wickX}
                y1={y}
                x2={wickX}
                y2={y + height}
                stroke={color}
                strokeWidth={1}
            />
            {/* Body */}
            {/* Ensure minimal height for visibility */}
            <rect
                x={x}
                y={bodyTop}
                width={width}
                height={Math.max(bodyHeight, 1)}
                fill={color}
            />
        </g>
    );
};

export default function CandlestickChart({
    data,
    width = "100%",
    height = 400,
    upColor = "#10B981",
    downColor = "#EF4444"
}: CandlestickChartProps) {

    // Transform data for Recharts Bar
    // We need data containing [low, high] for the Bar
    const processedData = data.map(d => ({
        ...d,
        range: [d.low, d.high] as [number, number]
    }));

    return (
        <div className="w-full" style={{ height }}>
            <ResponsiveContainer width={width} height="100%">
                <BarChart data={processedData} margin={{ top: 10, right: 30, left: 0, bottom: 0 }}>
                    <CartesianGrid strokeDasharray="3 3" stroke="#374151" vertical={false} />
                    <XAxis
                        dataKey="time"
                        stroke="#9CA3AF"
                        fontSize={11}
                        tickLine={false}
                        axisLine={false}
                    />
                    <YAxis
                        stroke="#9CA3AF"
                        fontSize={11}
                        tickLine={false}
                        axisLine={false}
                        domain={['auto', 'auto']}
                    />
                    <Tooltip
                        cursor={{ fill: '#1F2937', opacity: 0.4 }}
                        content={({ active, payload }) => {
                            if (active && payload && payload.length) {
                                const d = payload[0].payload;
                                return (
                                    <div className="bg-gray-800 border border-gray-700 p-2 rounded shadow-lg text-xs">
                                        <div className="text-gray-400 mb-1">{d.time}</div>
                                        <div className="grid grid-cols-2 gap-x-4 gap-y-1">
                                            <span className="text-gray-500">Open:</span>
                                            <span className="font-mono text-gray-300">{d.open.toFixed(2)}</span>
                                            <span className="text-gray-500">High:</span>
                                            <span className="font-mono text-gray-300">{d.high.toFixed(2)}</span>
                                            <span className="text-gray-500">Low:</span>
                                            <span className="font-mono text-gray-300">{d.low.toFixed(2)}</span>
                                            <span className="text-gray-500">Close:</span>
                                            <span className="font-mono text-gray-300">{d.close.toFixed(2)}</span>
                                        </div>
                                    </div>
                                );
                            }
                            return null;
                        }}
                    />
                    <Bar
                        dataKey="range"
                        shape={(props: any) => <CandlestickShape {...props} upColor={upColor} downColor={downColor} />}
                        animationDuration={300}
                    />
                </BarChart>
            </ResponsiveContainer>
        </div>
    )
}
