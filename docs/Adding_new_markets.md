Future Work

 These changes require more extensive refactoring and should be done in a separate phase:

 1. Create monitor_event function - Generic version of monitor_game for all market types
 2. Add ProviderRegistry - Abstract ESPN into EventProvider pattern
 3. Create EspnEventProvider - Wrap EspnClient in EventProvider trait
 4. Update DB writes - Support non-sport events in database inserts
 5. Extend TradingSignal - Add event_id and market_type fields