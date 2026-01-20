import { create } from 'zustand'
import { persist } from 'zustand/middleware'

interface UIPreferencesState {
  // Dashboard visibility
  showLiveGames: boolean
  showRecentTrades: boolean
  showOpportunities: boolean
  showEquityCurve: boolean
  showRiskBar: boolean
  showUpcomingGames: boolean

  // Analytics preferences
  analyticsTimePeriod: '7d' | '30d' | '90d' | 'all'

  // Risk display mode
  riskDisplayMode: 'compact' | 'detailed'
  showLatencyChart: boolean

  // Collapsed sections
  collapsedSections: Record<string, boolean>

  // Actions
  setShowLiveGames: (show: boolean) => void
  setShowRecentTrades: (show: boolean) => void
  setShowOpportunities: (show: boolean) => void
  setShowEquityCurve: (show: boolean) => void
  setShowRiskBar: (show: boolean) => void
  setShowUpcomingGames: (show: boolean) => void
  setAnalyticsTimePeriod: (period: '7d' | '30d' | '90d' | 'all') => void
  setRiskDisplayMode: (mode: 'compact' | 'detailed') => void
  setShowLatencyChart: (show: boolean) => void
  toggleSection: (sectionId: string) => void
  isSectionCollapsed: (sectionId: string) => boolean
}

export const useUIPreferences = create<UIPreferencesState>()(
  persist(
    (set, get) => ({
      // Default visibility - all shown
      showLiveGames: true,
      showRecentTrades: true,
      showOpportunities: true,
      showEquityCurve: true,
      showRiskBar: true,
      showUpcomingGames: true,

      // Default analytics period
      analyticsTimePeriod: '30d',

      // Default risk display
      riskDisplayMode: 'compact',
      showLatencyChart: true,

      // Collapsed sections
      collapsedSections: {},

      // Actions
      setShowLiveGames: (show) => set({ showLiveGames: show }),
      setShowRecentTrades: (show) => set({ showRecentTrades: show }),
      setShowOpportunities: (show) => set({ showOpportunities: show }),
      setShowEquityCurve: (show) => set({ showEquityCurve: show }),
      setShowRiskBar: (show) => set({ showRiskBar: show }),
      setShowUpcomingGames: (show) => set({ showUpcomingGames: show }),
      setAnalyticsTimePeriod: (period) => set({ analyticsTimePeriod: period }),
      setRiskDisplayMode: (mode) => set({ riskDisplayMode: mode }),
      setShowLatencyChart: (show) => set({ showLatencyChart: show }),
      toggleSection: (sectionId) =>
        set((state) => ({
          collapsedSections: {
            ...state.collapsedSections,
            [sectionId]: !state.collapsedSections[sectionId],
          },
        })),
      isSectionCollapsed: (sectionId) => get().collapsedSections[sectionId] || false,
    }),
    {
      name: 'arbees-ui-preferences',
    }
  )
)

// Helper hook for time period to days conversion
export function useTimePeriodDays(): number {
  const { analyticsTimePeriod } = useUIPreferences()
  switch (analyticsTimePeriod) {
    case '7d':
      return 7
    case '30d':
      return 30
    case '90d':
      return 90
    case 'all':
      return 365
    default:
      return 30
  }
}
