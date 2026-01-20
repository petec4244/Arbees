import { Routes, Route, Link } from 'react-router-dom'
import { useWebSocket } from './hooks/useWebSocket'
import Dashboard from './pages/Dashboard'
import Opportunities from './pages/Opportunities'
import LiveGames from './pages/LiveGames'
import PaperTrading from './pages/PaperTrading'
import SystemStatus from './components/SystemStatus'

function App() {
  const { isConnected, lastMessage } = useWebSocket()

  return (
    <div className="min-h-screen bg-gray-900 text-white">
      {/* Navigation */}
      <nav className="bg-gray-800 border-b border-gray-700">
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
          <div className="flex items-center justify-between h-16">
            <div className="flex items-center">
              <span className="text-xl font-bold text-green-400">Arbees</span>
              <div className="ml-10 flex items-baseline space-x-4">
                <Link to="/" className="px-3 py-2 rounded-md text-sm font-medium hover:bg-gray-700">
                  Dashboard
                </Link>
                <Link to="/opportunities" className="px-3 py-2 rounded-md text-sm font-medium hover:bg-gray-700">
                  Opportunities
                </Link>
                <Link to="/live-games" className="px-3 py-2 rounded-md text-sm font-medium hover:bg-gray-700">
                  Live Games
                </Link>
                <Link to="/paper-trading" className="px-3 py-2 rounded-md text-sm font-medium hover:bg-gray-700">
                  Paper Trading
                </Link>
              </div>
            </div>
            <div className="flex items-center space-x-4">
              <SystemStatus />
              <div className="flex items-center">
                <span className={`w-2 h-2 rounded-full mr-2 ${isConnected ? 'bg-green-400' : 'bg-red-400'}`} />
                <span className="text-sm text-gray-400">
                  {isConnected ? 'WS' : 'No WS'}
                </span>
              </div>
            </div>
          </div>
        </div>
      </nav>

      {/* Main Content */}
      <main className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 py-8">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/opportunities" element={<Opportunities />} />
          <Route path="/live-games" element={<LiveGames />} />
          <Route path="/paper-trading" element={<PaperTrading />} />
        </Routes>
      </main>
    </div>
  )
}

export default App
