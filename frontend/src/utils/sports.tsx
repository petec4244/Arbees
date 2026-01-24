import React from 'react';

export interface SportStyle {
    colors: string;
    badge: string;
    emoji: string;
}

export const SPORT_CONFIG: Record<string, SportStyle> = {
    nba: {
        colors: 'bg-orange-950/30 border-orange-500/20 hover:bg-orange-900/20',
        badge: 'bg-orange-900/50 text-orange-200 border-orange-700/50',
        emoji: '🏀'
    },
    nfl: {
        colors: 'bg-green-950/30 border-green-500/20 hover:bg-green-900/20',
        badge: 'bg-green-900/50 text-green-200 border-green-700/50',
        emoji: '🏈'
    },
    nhl: {
        colors: 'bg-cyan-950/30 border-cyan-500/20 hover:bg-cyan-900/20',
        badge: 'bg-cyan-900/50 text-cyan-200 border-cyan-700/50',
        emoji: '🏒'
    },
    mlb: {
        colors: 'bg-red-950/30 border-red-500/20 hover:bg-red-900/20',
        badge: 'bg-red-900/50 text-red-200 border-red-700/50',
        emoji: '⚾'
    },
    ncaaf: {
        colors: 'bg-purple-950/30 border-purple-500/20 hover:bg-purple-900/20',
        badge: 'bg-purple-900/50 text-purple-200 border-purple-700/50',
        emoji: '🏈'
    },
    ncaab: {
        colors: 'bg-amber-950/30 border-amber-500/20 hover:bg-amber-900/20',
        badge: 'bg-amber-900/50 text-amber-200 border-amber-700/50',
        emoji: '🏀'
    },
    soccer: {
        colors: 'bg-emerald-950/30 border-emerald-500/20 hover:bg-emerald-900/20',
        badge: 'bg-emerald-900/50 text-emerald-200 border-emerald-700/50',
        emoji: '⚽'
    },
    mls: {
        colors: 'bg-teal-950/30 border-teal-500/20 hover:bg-teal-900/20',
        badge: 'bg-teal-900/50 text-teal-200 border-teal-700/50',
        emoji: '⚽'
    },
    ufc: {
        colors: 'bg-rose-950/30 border-rose-500/20 hover:bg-rose-900/20',
        badge: 'bg-rose-900/50 text-rose-200 border-rose-700/50',
        emoji: '🥊'
    },
    mma: {
        colors: 'bg-rose-950/30 border-rose-500/20 hover:bg-rose-900/20',
        badge: 'bg-rose-900/50 text-rose-200 border-rose-700/50',
        emoji: '🥊'
    },
    tennis: {
        colors: 'bg-lime-950/30 border-lime-500/20 hover:bg-lime-900/20',
        badge: 'bg-lime-900/50 text-lime-200 border-lime-700/50',
        emoji: '🎾'
    },
    golf: {
        colors: 'bg-green-950/30 border-green-500/20 hover:bg-green-900/20',
        badge: 'bg-green-900/50 text-green-200 border-green-700/50',
        emoji: '⛳'
    },
    boxing: {
        colors: 'bg-red-950/30 border-red-500/20 hover:bg-red-900/20',
        badge: 'bg-red-900/50 text-red-200 border-red-700/50',
        emoji: '🥊'
    },
    cricket: {
        colors: 'bg-blue-950/30 border-blue-500/20 hover:bg-blue-900/20',
        badge: 'bg-blue-900/50 text-blue-200 border-blue-700/50',
        emoji: '🏏'
    },
    default: {
        colors: 'bg-gray-800 border-gray-700 hover:bg-gray-750',
        badge: 'bg-gray-700 text-gray-300 border-gray-600',
        emoji: '🏆'
    }
};

export function getSportConfig(sport: string): SportStyle {
    if (!sport) return SPORT_CONFIG.default;
    return SPORT_CONFIG[sport.toLowerCase()] || SPORT_CONFIG.default;
}

export function SportBackground({ sport }: { sport: string }) {
    const config = getSportConfig(sport);

    return (
        <div className="absolute -bottom-4 -right-4 opacity-5 pointer-events-none select-none overflow-hidden z-0">
            <span className="text-9xl grayscale transition-all duration-500 group-hover:grayscale-0 group-hover:scale-110 group-hover:opacity-10 filter">
                {config.emoji}
            </span>
        </div>
    );
}
