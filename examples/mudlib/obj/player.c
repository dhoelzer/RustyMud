// basic player object with a few commands

string name_of_player = "Stranger";
int done = 0;

void create()
{
}

int logon()
{
    write("\nWelcome to the ldmud-rs driver!\n");
    write("What is your name? ");
    input_to("read_name", 1);
    return 1;
}

void read_name(string str)
{
    if (str == "")
    {
        write("Please enter a name.\n");
        write("What is your name? ");
        input_to("read_name", 1);
        return;
    }
    name_of_player = capitalize(str);
    write("Hello, " + name_of_player + ". Here is a test mud.\n");
    write("Type 'help' for commands.\n");
    set_prompt(#'prompt);
    enable_commands();
    add_action("look_cmd", "look", 4);
    add_action("help_cmd", "help", 4);
    add_action("quit_cmd", "quit", 1);
    add_action("echo_cmd", "echo", 4);
    add_action("time_cmd", "time", 4);
    add_action("eval_cmd", "eval", 4);
}

string prompt()
{
    return name_of_player + "> ";
}

int look_cmd(string str)
{
    write("You are sitting in a bare room lit by a soft glow.\n");
    write("Exits: none. Objects: " + name_of_player + " (you).\n");
    return 1;
}

int help_cmd(string str)
{
    write("Commands: look, help, echo <str>, time, eval <expr>, quit\n");
    return 1;
}

int quit_cmd(string str)
{
    write("Goodbye.\n");
    remove_interactive(1);
    destruct(this_object());
    return 1;
}

int echo_cmd(string str)
{
    if (str == "")
    {
        notify_fail("Echo what?\n");
        return 0;
    }
    write(str + "\n");
    return 1;
}

int time_cmd(string str)
{
    write(ctime(time()) + "\n");
    return 1;
}

// exercise the expression machinery: compile a driver eval request
int eval_cmd(string str)
{
    if (str == "")
    {
        notify_fail("Eval what expression?\n");
        return 0;
    }
    object ob = compile_string("mixed __t() { return (" + str + "); }");
    if (!ob)
    {
        write("compile failed\n");
        return 1;
    }
    mixed r = ob->__t();
    write(sprintf("%O\n", r));
    destruct(ob);
    return 1;
}

void disconnect()
{
}
