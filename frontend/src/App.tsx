import { useState } from 'react'
import { Routes, Route, Link, useLocation } from 'react-router-dom'
import { Menu, X } from 'lucide-react'
import { useWebSocket } from './hooks/useWebSocket'
import Dashboard from './pages/Dashboard'
import Analytics from './pages/Analytics'
import Opportunities from './pages/Opportunities'
import LiveGames from './pages/LiveGames'
import UpcomingGames from './pages/UpcomingGames'
import PaperTrading from './pages/PaperTrading'
import HistoricalGames from './pages/HistoricalGames'
import SystemStatus from './components/SystemStatus'

function NavLink({ to, children, onClick }: { to: string; children: React.ReactNode; onClick?: () => void }) {
  const location = useLocation()
  const isActive = location.pathname === to || (to !== '/' && location.pathname.startsWith(to))

  return (
    <Link
      to={to}
      onClick={onClick}
      className={`px-3 py-2 rounded-md text-sm font-medium transition-colors ${
        isActive
          ? 'bg-gray-700 text-white'
          : 'text-gray-300 hover:bg-gray-700 hover:text-white'
      }`}
    >
      {children}
    </Link>
  )
}

function MobileNavLink({ to, children, onClick }: { to: string; children: React.ReactNode; onClick?: () => void }) {
  const location = useLocation()
  const isActive = location.pathname === to || (to !== '/' && location.pathname.startsWith(to))

  return (
    <Link
      to={to}
      onClick={onClick}
      className={`block px-3 py-3 rounded-md text-base font-medium transition-colors ${
        isActive
          ? 'bg-gray-700 text-white'
          : 'text-gray-300 hover:bg-gray-700 hover:text-white'
      }`}
    >
      {children}
    </Link>
  )
}

function App() {
  const { isConnected } = useWebSocket()
  const [mobileMenuOpen, setMobileMenuOpen] = useState(false)

  const closeMobileMenu = () => setMobileMenuOpen(false)

  return (
    <div className="min-h-screen bg-gray-900 text-white">
      {/* Navigation */}
      <nav className="bg-gray-800 border-b border-gray-700 sticky top-0 z-50">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-16">
            <div className="flex items-center">
              {/* Mobile menu button */}
              <button
                onClick={() => setMobileMenuOpen(!mobileMenuOpen)}
                className="md:hidden p-2 rounded-md text-gray-400 hover:text-white hover:bg-gray-700 focus:outline-none"
              >
                {mobileMenuOpen ? (
                  <X className="w-6 h-6" />
                ) : (
                  <Menu className="w-6 h-6" />
                )}
              </button>

              <Link to="/" className="text-xl font-bold text-green-400 hover:text-green-300 transition-colors ml-2 md:ml-0">
                Arbees
              </Link>

              {/* Desktop navigation */}
              <div className="hidden md:flex ml-10 items-baseline space-x-1">
                <NavLink to="/">Dashboard</NavLink>
                <NavLink to="/analytics">Analytics</NavLink>
                <NavLink to="/opportunities">Opportunities</NavLink>
                <NavLink to="/live-games">Live Games</NavLink>
                <NavLink to="/upcoming-games">Upcoming</NavLink>
                <NavLink to="/historical">Historical</NavLink>
                <NavLink to="/paper-trading">Paper Trading</NavLink>
              </div>
            </div>

            <div className="flex items-center space-x-2 sm:space-x-4">
              <div className="hidden sm:block">
                <SystemStatus />
              </div>
              <div className="flex items-center">
                <span className={`w-2 h-2 rounded-full mr-2 ${isConnected ? 'bg-green-400' : 'bg-red-400'}`} />
                <span className="text-xs sm:text-sm text-gray-400">
                  {isConnected ? 'WS' : 'No WS'}
                </span>
              </div>
            </div>
          </div>
        </div>

        {/* Mobile navigation menu */}
        {mobileMenuOpen && (
          <div className="md:hidden bg-gray-800 border-t border-gray-700">
            <div className="px-2 pt-2 pb-3 space-y-1">
              <MobileNavLink to="/" onClick={closeMobileMenu}>Dashboard</MobileNavLink>
              <MobileNavLink to="/analytics" onClick={closeMobileMenu}>Analytics</MobileNavLink>
              <MobileNavLink to="/opportunities" onClick={closeMobileMenu}>Opportunities</MobileNavLink>
              <MobileNavLink to="/live-games" onClick={closeMobileMenu}>Live Games</MobileNavLink>
              <MobileNavLink to="/upcoming-games" onClick={closeMobileMenu}>Upcoming Games</MobileNavLink>
              <MobileNavLink to="/historical" onClick={closeMobileMenu}>Historical Games</MobileNavLink>
              <MobileNavLink to="/paper-trading" onClick={closeMobileMenu}>Paper Trading</MobileNavLink>
            </div>
            <div className="px-4 py-3 border-t border-gray-700">
              <SystemStatus />
            </div>
          </div>
        )}
      </nav>

      {/* Main Content */}
      <main className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-4 sm:py-8">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/analytics" element={<Analytics />} />
          <Route path="/opportunities" element={<Opportunities />} />
          <Route path="/live-games" element={<LiveGames />} />
          <Route path="/upcoming-games" element={<UpcomingGames />} />
          <Route path="/historical" element={<HistoricalGames />} />
          <Route path="/paper-trading" element={<PaperTrading />} />
        </Routes>
      </main>
    </div>
  )
}

export default App
