Scriptname SkyrimRsTestRef extends ObjectReference Hidden
{A method native and an event; the test plugin binds an object of this script to the player, so DevBench and a dispatch can call it and SendEvent reaches it.}

int Function SelfFormId() native

Event OnSkyrimRsEvent(int count, string text)
    SkyrimRsTest.RecordEvent(count, text)
EndEvent
