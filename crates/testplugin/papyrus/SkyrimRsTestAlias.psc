Scriptname SkyrimRsTestAlias extends ReferenceAlias Hidden
{The test plugin binds an object of this script to one of the player's quest aliases, which the event relay reaches.}

Event OnSkyrimRsEvent(int count, string text)
    SkyrimRsTest.RecordRelay("alias", count)
EndEvent
