Action 2: Add Detailed Logging to Every Trade
We need to see EXACTLY what's happening. Add this to position_manager:
pythonasync def _execute_signal(self, signal: TradingSignal):
    # BEFORE execution
    logger.critical(
        f"🚨 SIGNAL RECEIVED:\n"
        f"  Game: {signal.game_id}\n"
        f"  Platform: {signal.platform}\n"
        f"  Direction: {signal.direction}\n"
        f"  Signal Type: {signal.signal_type}\n"
        f"  Expected Price: {signal.expected_price:.2f}¢\n"
        f"  Market Price: {signal.market_price:.2f}¢\n"
        f"  Edge: {signal.edge:.2f}%\n"
        f"  Win Prob: {signal.win_probability:.2f}%\n"
        f"  Timestamp: {signal.timestamp}"
    )
    
    # Execute
    trade = await self.paper_trader.execute(signal)
    
    # AFTER execution
    logger.critical(
        f"🚨 TRADE EXECUTED:\n"
        f"  Trade ID: {trade.trade_id}\n"
        f"  Status: {trade.status}\n"
        f"  Entry Price: {trade.entry_price:.2f}¢\n"
        f"  Contracts: {trade.contracts}\n"
        f"  Cost: ${trade.cost:.2f}\n"
        f"  Platform: {trade.platform}"
    )
Run for 1 hour and capture every signal + execution.

Action 3: Check Exit Monitor Logic
Your exit monitor might be closing positions immediately. Check this:
python# In position_manager.py, find exit_monitor code

async def _check_exit_conditions(self, position):
    current_prob = # ... get current win prob
    entry_prob = position.entry_win_prob
    
    prob_change = abs(current_prob - entry_prob)
    
    # THIS MIGHT BE TOO AGGRESSIVE!
    if prob_change > self.stop_loss_threshold:
        logger.warning(
            f"🛑 EXIT: Prob changed {prob_change:.1f}% "
            f"(threshold: {self.stop_loss_threshold:.1f}%)"
        )
        await self._close_position(position)
Check:

What's the stop_loss_threshold? (Should be 3-7%, not 0.5%)
Is it checking direction correctly? (should only exit if moved AGAINST us)
Is it using fresh data? (or stale win prob?)


Action 4: Verify Win Probability Calculation
Bad win probability = bad signals. Test this:
python# Create test script
async def test_win_prob():
    espn = ESPNClient()
    state = await espn.get_game_state("401734294")  # Real game ID
    
    # Calculate win prob
    win_prob = arbees_core.calculate_win_probability(
        sport=state.sport.value,
        home_score=state.home_score,
        away_score=state.away_score,
        game_clock=state.game_clock,
        period=state.period,
    )
    
    print(f"Game State:")
    print(f"  Score: {state.home_score} - {state.away_score}")
    print(f"  Clock: {state.game_clock}")
    print(f"  Period: {state.period}")
    print(f"  Win Prob: {win_prob:.2f}%")
    
    # Sanity check
    assert 0 <= win_prob <= 100, "Win prob out of range!"
    
    if state.home_score > state.away_score:
        assert win_prob > 50, "Home winning but prob < 50%?"

asyncio.run(test_win_prob())

Action 5: Check for Race Conditions
With 44,000 lines of Python, you might have:
Duplicate position tracking:
python# Position opened in game_shard
position = await self._open_position(signal)

# BUT position_manager ALSO tracking?
# Now you have TWO positions for same signal!
Check:

Are multiple shards trading the same game?
Is position deduplication working?
Are you using Redis locks correctly?


Action 6: Review Recent Code Changes
You said: "every edge case we create when we add new things/safeguards"
What did you add recently?

New exit logic?
New stop-loss thresholds?
New signal filters?
New market types?