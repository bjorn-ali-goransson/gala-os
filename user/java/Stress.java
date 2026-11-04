// Runs javac in-process many times, three compilations at a time, compiling
// Http.java (the `java_stress` boot scenario, pending: a stress run for the
// Java heap corruption, WORKFLOW "Known intermittent"). A crash (javac's
// "An exception has occurred in the compiler") counts as a failure.
// Arguments: compilations (default 12), then optionally the interval in
// ms of full collections forced meanwhile (System.gc()).
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.Future;
import javax.tools.JavaCompiler;
import javax.tools.ToolProvider;

public class Stress {
    public static void main(String[] args) throws Exception {
        int rounds = args.length > 0 ? Integer.parseInt(args[0]) : 12;
        // A full collection every `gcEvery` ms meanwhile (0: none): full
        // collections move every object, and the corruption followed one.
        long gcEvery = args.length > 1 ? Long.parseLong(args[1]) : 0;
        if (gcEvery > 0) {
            Thread gc = new Thread(() -> {
                while (true) {
                    try {
                        Thread.sleep(gcEvery);
                    } catch (InterruptedException e) {
                        return;
                    }
                    System.gc();
                }
            });
            gc.setDaemon(true);
            gc.start();
        }
        JavaCompiler javac = ToolProvider.getSystemJavaCompiler();
        ExecutorService pool = Executors.newFixedThreadPool(3);
        List<Future<Integer>> results = new ArrayList<>();
        for (int i = 0; i < rounds; i++) {
            String out = "C:/stress/" + i;
            results.add(pool.submit(() -> javac.run(null, null, null, "-d", out, "C:/gala/Http.java")));
        }
        int failed = 0;
        for (Future<Integer> f : results) {
            if (f.get() != 0) {
                failed++;
            }
        }
        pool.shutdown();
        System.out.println("java_stress: " + rounds + " compiles, " + failed + " failed");
    }
}
