Scriptname SkyrimRsTestEffect extends ActiveMagicEffect Hidden
{The test plugin binds an object of this script to one of the player's scripted active effects, which the event relay reaches.}

Event OnSkyrimRsEvent(int count, string text)
    SkyrimRsTest.RecordRelay("effect", count)
EndEvent
