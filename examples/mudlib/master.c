// minimal master object for ldmud-rs
// Master applies used by the driver: connect, compile_object, valid_read,
// valid_write, remove_player, get_simul_efun, runtime_error, shutdown.

#pragma no_clone

string *files = ({ });

object connect()
{
    object ob;

    ob = clone_object("/obj/player");
    if (!ob)
        return 0;
    return ob;
}

int compile_object(string path)
{
    return 1;
}

int valid_read(string file, object ob)
{
    return 1;
}

int valid_write(string file, object ob)
{
    if (file_name(ob) == "/obj/test")
        return 1;
    return 1;
}

void remove_player(object player)
{
}

mixed get_simul_efun()
{
    return 0;
}

int runtime_error(string err)
{
    write("*ERROR: " + err + "\n");
    return 1;
}

void shutdown()
{
    write("Master says good bye.\n");
}
