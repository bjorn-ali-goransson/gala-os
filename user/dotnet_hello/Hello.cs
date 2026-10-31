// A managed test program for gala's .NET bring-up: what CoreCLR needs to
// start, JIT, collect garbage, throw and catch (also from a hardware
// fault), and run a second thread. Expected output on .NET 8.0.33 on
// Windows 11 24H2 (build 26100) with two processors:
//
//   hello from .NET 8.0.33
//   os=Microsoft Windows NT 10.0.26100.0
//   64bit=True processors=2
//   the=3
//   sum=499999500000
//   caught NullReferenceException
//   caught boom
//   thread=42
//   gc=ok
//   pi=3.14159
//
// Built by build.sh (Mono's mcs, a portable assembly referencing
// mscorlib; .NET forwards its types).
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Threading;

class Hello
{
    static int Main()
    {
        Console.WriteLine("hello from .NET " + Environment.Version);
        Console.WriteLine("os=" + Environment.OSVersion);
        Console.WriteLine("64bit=" + Environment.Is64BitProcess + " processors=" + Environment.ProcessorCount);

        var counts = new Dictionary<string, int>();
        foreach (var w in "the quick brown fox jumps over the lazy dog the end the".Split(' '))
        {
            int n;
            counts.TryGetValue(w, out n);
            counts[w] = n + 1;
        }
        Console.WriteLine("the=" + counts["the"]);

        long sum = 0;
        for (int i = 0; i < 1000000; i++)
            sum += i;
        Console.WriteLine("sum=" + sum);

        try
        {
            object o = null;
            Console.WriteLine(o.ToString());
        }
        catch (NullReferenceException)
        {
            Console.WriteLine("caught NullReferenceException");
        }
        try
        {
            throw new InvalidOperationException("boom");
        }
        catch (Exception e)
        {
            Console.WriteLine("caught " + e.Message);
        }

        int result = 0;
        var t = new Thread(() => { result = 42; });
        t.Start();
        t.Join();
        Console.WriteLine("thread=" + result);

        var keep = new List<byte[]>();
        for (int i = 0; i < 2000; i++)
        {
            var b = new byte[16 * 1024];
            b[i % b.Length] = 1;
            if (i % 100 == 0)
                keep.Add(b);
        }
        GC.Collect();
        Console.WriteLine("gc=" + (keep.Count == 20 && GC.CollectionCount(0) > 0 ? "ok" : "bad"));

        Console.WriteLine(string.Format(CultureInfo.InvariantCulture, "pi={0:F5}", Math.PI));
        return 0;
    }
}
